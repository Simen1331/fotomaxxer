//! HDBSCAN portert fra scikit-learn 1.9 (`sklearn/cluster/_hdbscan/`).
//!
//! Python-versjonen kaller `HDBSCAN(min_cluster_size=m, metric="euclidean").fit_predict(X)`
//! med standardverdiene `min_samples=None` (= `m`), `alpha=1.0`, `cluster_selection_method="eom"`,
//! `allow_single_cluster=False`, `cluster_selection_epsilon=0.0`. Med euklidsk metrikk
//! velger sklearn `algorithm="auto"` → KD-tre for kjerneavstander og Prims algoritme
//! (`mst_from_data_matrix`) for MST-en. Kravet er **identiske etiketter**, også
//! nummereringen (personer opprettes i etikettrekkefølge), så hvert steg følger
//! originalen tett:
//!
//! 1. `X` som `float64`. Avstand = `sqrt(Σ (a_j − b_j)²)` summert sekvensielt i `f64`,
//!    som `euclidean_dist64` (samme verdi som KD-treet gir, så kjerneavstanden blir lik).
//! 2. Kjerneavstand = avstanden til den `min_samples`-te nærmeste (punktet selv inkludert).
//! 3. Prims MST med gjensidig nåbarhet `max(core_i, core_j, d_ij)`, med samme
//!    oppdateringsregler og likhetsbrudd som Cython-koden.
//! 4. Kantene sorteres med [`npsort::argsort`] (numpys ikke-stabile rekkefølge for like
//!    avstander), og enkeltlenke-treet bygges med union-find.
//! 5. Kondensert tre, stabilitet, EOM-utvalg og merking som `_tree.pyx`.

use rayon::prelude::*;

use crate::npsort;

/// Støy-etikett.
pub const NOISE: i64 = -1;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum HdbscanError {
    #[error(
        "The 'min_cluster_size' parameter of HDBSCAN must be an int in the range [2, inf). Got {0} instead."
    )]
    MinClusterSize(usize),
    #[error("n_samples=1 while HDBSCAN requires more than one sample")]
    SingleSample,
    #[error("min_samples ({0}) must be at most the number of samples in X ({1})")]
    MinSamples(usize, usize),
    #[error("alle punkter må ha samme dimensjon")]
    Dimension,
    #[error("Input contains NaN or infinity.")]
    NonFinite,
}

/// `HDBSCAN(min_cluster_size).fit_predict(points)` → én etikett per punkt (−1 = støy).
pub fn fit_predict(points: &[&[f32]], min_cluster_size: usize) -> Result<Vec<i64>, HdbscanError> {
    if min_cluster_size < 2 {
        return Err(HdbscanError::MinClusterSize(min_cluster_size));
    }
    let n = points.len();
    if n == 0 {
        return Ok(Vec::new());
    }
    let dim = points[0].len();
    if points.iter().any(|p| p.len() != dim) {
        return Err(HdbscanError::Dimension);
    }
    if points.iter().flat_map(|p| p.iter()).any(|v| !v.is_finite()) {
        return Err(HdbscanError::NonFinite);
    }
    if n == 1 {
        return Err(HdbscanError::SingleSample);
    }
    let min_samples = min_cluster_size;
    if min_samples > n {
        return Err(HdbscanError::MinSamples(min_samples, n));
    }
    let data = Data::new(points, dim);
    let core = core_distances(&data, min_samples);
    let mst = prim_mst(&data, &core);
    let slt = single_linkage(&mst);
    let condensed = condense_tree(&slt, min_cluster_size);
    let stability = compute_stability(&condensed);
    Ok(get_clusters(&condensed, stability))
}

/// Punktene som en tett `n × dim`-matrise i `f64` (`validate_data(dtype=np.float64)`).
struct Data {
    values: Vec<f64>,
    dim: usize,
}

impl Data {
    fn new(points: &[&[f32]], dim: usize) -> Self {
        let values = points
            .iter()
            .flat_map(|p| p.iter().map(|&v| v as f64))
            .collect();
        Self { values, dim }
    }

    fn len(&self) -> usize {
        self.values.len() / self.dim.max(1)
    }

    fn row(&self, i: usize) -> &[f64] {
        &self.values[i * self.dim..(i + 1) * self.dim]
    }

    /// `euclidean_dist64`: sekvensiell sum i `f64`, så `sqrt`.
    fn dist(&self, i: usize, j: usize) -> f64 {
        let (a, b) = (self.row(i), self.row(j));
        let mut d = 0.0f64;
        for k in 0..a.len() {
            let t = a[k] - b[k];
            d += t * t;
        }
        d.sqrt()
    }
}

/// Avstand til den `k`-te nærmeste (punktet selv teller med, avstand 0),
/// som `NearestNeighbors(n_neighbors=k).kneighbors(X)[0][:, -1]`.
fn core_distances(data: &Data, k: usize) -> Vec<f64> {
    let n = data.len();
    (0..n)
        .into_par_iter()
        .map(|i| {
            let mut d: Vec<f64> = (0..n).map(|j| data.dist(i, j)).collect();
            let (_, kth, _) = d.select_nth_unstable_by(k - 1, f64::total_cmp);
            *kth
        })
        .collect()
}

#[derive(Debug, Clone, Copy)]
struct Edge {
    current: usize,
    next: usize,
    distance: f64,
}

/// `mst_from_data_matrix` (Prim, alpha = 1): samme rekkefølge og likhetsregler.
fn prim_mst(data: &Data, core: &[f64]) -> Vec<Edge> {
    let n = data.len();
    let mut in_tree = vec![false; n];
    let mut min_reach = vec![f64::INFINITY; n];
    let mut sources = vec![1usize; n];
    let mut pair = vec![0.0f64; n];
    let mut mst = Vec::with_capacity(n - 1);
    let mut current = 0usize;
    // Parvise avstander regnes parallelt (dyrt: n·dim), oppdateringen sekvensielt
    // (billig, og rekkefølgen avgjør likhetsbrudd).
    let parallel = n * data.dim >= 1 << 18;
    for _ in 0..n - 1 {
        in_tree[current] = true;
        if parallel {
            pair.par_iter_mut().enumerate().for_each(|(j, p)| {
                if !in_tree[j] {
                    *p = data.dist(current, j);
                }
            });
        } else {
            for j in 0..n {
                if !in_tree[j] {
                    pair[j] = data.dist(current, j);
                }
            }
        }
        let current_core = core[current];
        let mut new_reach = f64::MAX;
        let (mut source, mut new_node) = (0usize, 0usize);
        for j in 0..n {
            if in_tree[j] {
                continue;
            }
            let next_min = min_reach[j];
            let next_source = sources[j];
            // Cythons max(a, b, c) for C-double.
            let mut mrd = current_core;
            if core[j] > mrd {
                mrd = core[j];
            }
            if pair[j] > mrd {
                mrd = pair[j];
            }
            if mrd < next_min {
                min_reach[j] = mrd;
                sources[j] = current;
                if mrd < new_reach {
                    new_reach = mrd;
                    source = current;
                    new_node = j;
                }
            } else if next_min < new_reach {
                new_reach = next_min;
                source = next_source;
                new_node = j;
            }
        }
        mst.push(Edge {
            current: source,
            next: new_node,
            distance: new_reach,
        });
        current = new_node;
    }
    mst
}

/// Én rad i enkeltlenke-treet (scipy-format): `left`, `right`, avstand, størrelse.
#[derive(Debug, Clone, Copy)]
struct Merge {
    left: usize,
    right: usize,
    value: f64,
    size: usize,
}

/// `_process_mst` + `make_single_linkage`.
fn single_linkage(mst: &[Edge]) -> Vec<Merge> {
    let n = mst.len() + 1;
    let distances: Vec<f64> = mst.iter().map(|e| e.distance).collect();
    let order = npsort::argsort(&distances);
    let mut parent = vec![usize::MAX; 2 * n - 1];
    let mut size: Vec<usize> = (0..2 * n - 1).map(|i| usize::from(i < n)).collect();
    let mut next_label = n;
    let find = |parent: &mut Vec<usize>, mut x: usize| {
        let start = x;
        while parent[x] != usize::MAX {
            x = parent[x];
        }
        // Stikomprimering (påvirker ikke resultatet).
        let mut p = start;
        while parent[p] != usize::MAX && parent[p] != x {
            let up = parent[p];
            parent[p] = x;
            p = up;
        }
        x
    };
    let mut out = Vec::with_capacity(n - 1);
    for &e in order.iter().map(|&i| &mst[i]) {
        let a = find(&mut parent, e.current);
        let b = find(&mut parent, e.next);
        out.push(Merge {
            left: a,
            right: b,
            value: e.distance,
            size: size[a] + size[b],
        });
        parent[a] = next_label;
        parent[b] = next_label;
        size[next_label] = size[a] + size[b];
        next_label += 1;
    }
    out
}

/// `bfs_from_hierarchy`: bredde-først fra `root` (blader og indre noder).
fn bfs_hierarchy(h: &[Merge], root: usize) -> Vec<usize> {
    let n = h.len() + 1;
    let mut result = Vec::new();
    let mut queue = vec![root];
    while !queue.is_empty() {
        result.extend_from_slice(&queue);
        let inner: Vec<usize> = queue.iter().filter(|&&x| x >= n).map(|&x| x - n).collect();
        queue = inner
            .iter()
            .flat_map(|&node| [h[node].left, h[node].right])
            .collect();
    }
    result
}

/// Rad i det kondenserte treet.
#[derive(Debug, Clone, Copy)]
struct Condensed {
    parent: usize,
    child: usize,
    lambda: f64,
    size: usize,
}

/// `_condense_tree`.
fn condense_tree(h: &[Merge], min_cluster_size: usize) -> Vec<Condensed> {
    let root = 2 * h.len();
    let n = h.len() + 1;
    let mut next_label = n + 1;
    let node_list = bfs_hierarchy(h, root);
    let mut relabel = vec![0usize; root + 1];
    relabel[root] = n;
    let mut ignore = vec![false; node_list.len()];
    let mut out = Vec::new();

    let count = |node: usize| if node >= n { h[node - n].size } else { 1 };
    let fall_out = |out: &mut Vec<Condensed>, ignore: &mut [bool], parent, sub_root, lambda| {
        for sub in bfs_hierarchy(h, sub_root) {
            if sub < n {
                out.push(Condensed {
                    parent,
                    child: sub,
                    lambda,
                    size: 1,
                });
            }
            ignore[sub] = true;
        }
    };

    for &node in &node_list {
        if ignore[node] || node < n {
            continue;
        }
        let Merge {
            left,
            right,
            value: distance,
            ..
        } = h[node - n];
        let lambda = if distance > 0.0 {
            1.0 / distance
        } else {
            f64::INFINITY
        };
        let (left_count, right_count) = (count(left), count(right));
        if left_count >= min_cluster_size && right_count >= min_cluster_size {
            relabel[left] = next_label;
            next_label += 1;
            out.push(Condensed {
                parent: relabel[node],
                child: relabel[left],
                lambda,
                size: left_count,
            });
            relabel[right] = next_label;
            next_label += 1;
            out.push(Condensed {
                parent: relabel[node],
                child: relabel[right],
                lambda,
                size: right_count,
            });
        } else if left_count < min_cluster_size && right_count < min_cluster_size {
            fall_out(&mut out, &mut ignore, relabel[node], left, lambda);
            fall_out(&mut out, &mut ignore, relabel[node], right, lambda);
        } else if left_count < min_cluster_size {
            relabel[right] = relabel[node];
            fall_out(&mut out, &mut ignore, relabel[node], left, lambda);
        } else {
            relabel[left] = relabel[node];
            fall_out(&mut out, &mut ignore, relabel[node], right, lambda);
        }
    }
    out
}

/// Stabilitet per klynge-id (`_compute_stability`), indeksert fra den minste (roten).
struct Stability {
    smallest: usize,
    values: Vec<f64>,
}

fn compute_stability(tree: &[Condensed]) -> Stability {
    let smallest = tree.iter().map(|r| r.parent).min().unwrap_or(0);
    let largest_parent = tree.iter().map(|r| r.parent).max().unwrap_or(0);
    let largest_child = tree
        .iter()
        .map(|r| r.child)
        .max()
        .unwrap_or(0)
        .max(smallest);
    let mut births = vec![f64::NAN; largest_child + 1];
    for r in tree {
        births[r.child] = r.lambda;
    }
    births[smallest] = 0.0;
    let mut values = vec![0.0f64; largest_parent - smallest + 1];
    for r in tree {
        values[r.parent - smallest] += (r.lambda - births[r.parent]) * r.size as f64;
    }
    Stability { smallest, values }
}

/// `bfs_from_cluster_tree`: klynge-id-ene under (og inkludert) `root`.
fn bfs_cluster_tree(cluster_tree: &[Condensed], root: usize) -> Vec<usize> {
    let mut result = Vec::new();
    let mut queue = vec![root];
    while !queue.is_empty() {
        result.extend_from_slice(&queue);
        queue = cluster_tree
            .iter()
            .filter(|r| queue.contains(&r.parent))
            .map(|r| r.child)
            .collect();
    }
    result
}

/// `_get_clusters` med EOM, `allow_single_cluster=False`, epsilon 0 → etiketter.
fn get_clusters(tree: &[Condensed], stability: Stability) -> Vec<i64> {
    let Stability {
        smallest: root,
        values: mut stab,
    } = stability;
    let cluster_tree: Vec<Condensed> = tree.iter().copied().filter(|r| r.size > 1).collect();
    // Synkende klynge-id uten roten (ID-ene er en topologisk sortering av treet).
    let node_list: Vec<usize> = (root + 1..root + stab.len()).rev().collect();
    let mut is_cluster = vec![false; stab.len()];
    for &node in &node_list {
        is_cluster[node - root] = true;
    }
    for &node in &node_list {
        let children = cluster_tree.iter().filter(|r| r.parent == node);
        // np.sum over 0 eller 2 barn (en klynge splittes alltid i to).
        let subtree: f64 = children.fold(0.0, |acc, r| acc + stab[r.child - root]);
        if subtree > stab[node - root] {
            is_cluster[node - root] = false;
            stab[node - root] = subtree;
        } else {
            for sub in bfs_cluster_tree(&cluster_tree, node) {
                if sub != node {
                    is_cluster[sub - root] = false;
                }
            }
        }
    }
    // Etikett = rang blant de valgte klynge-id-ene (sortert stigende).
    let mut label_of = vec![NOISE; stab.len()];
    let mut next = 0i64;
    for (offset, &selected) in is_cluster.iter().enumerate() {
        if selected {
            label_of[offset] = next;
            next += 1;
        }
    }
    do_labelling(tree, root, &is_cluster, &label_of)
}

/// `TreeUnionFind` (union by rank + stikomprimering), som i `_tree.pyx`.
struct TreeUnionFind {
    parent: Vec<usize>,
    rank: Vec<usize>,
}

impl TreeUnionFind {
    fn new(size: usize) -> Self {
        Self {
            parent: (0..size).collect(),
            rank: vec![0; size],
        }
    }

    fn find(&mut self, x: usize) -> usize {
        let mut root = x;
        while self.parent[root] != root {
            root = self.parent[root];
        }
        let mut p = x;
        while self.parent[p] != root {
            let up = self.parent[p];
            self.parent[p] = root;
            p = up;
        }
        root
    }

    fn union(&mut self, x: usize, y: usize) {
        let (xr, yr) = (self.find(x), self.find(y));
        if self.rank[xr] < self.rank[yr] {
            self.parent[xr] = yr;
        } else if self.rank[xr] > self.rank[yr] {
            self.parent[yr] = xr;
        } else {
            self.parent[yr] = xr;
            self.rank[xr] += 1;
        }
    }
}

/// `_do_labelling` (uten `allow_single_cluster`).
fn do_labelling(
    tree: &[Condensed],
    root: usize,
    is_cluster: &[bool],
    label_of: &[i64],
) -> Vec<i64> {
    let selected = |c: usize| c >= root && is_cluster[c - root];
    let largest = tree
        .iter()
        .map(|r| r.parent.max(r.child))
        .max()
        .unwrap_or(root);
    let mut uf = TreeUnionFind::new(largest + 1);
    for r in tree {
        if !selected(r.child) {
            uf.union(r.parent, r.child);
        }
    }
    (0..root)
        .map(|point| {
            let cluster = uf.find(point);
            if cluster == root {
                NOISE
            } else {
                // Representanten er alltid en valgt klynge (foreldre før barn i treet).
                label_of[cluster - root]
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(points: &[Vec<f32>], mcs: usize) -> Vec<i64> {
        let refs: Vec<&[f32]> = points.iter().map(Vec::as_slice).collect();
        fit_predict(&refs, mcs).unwrap()
    }

    #[test]
    fn to_tydelige_klynger_og_stoy() {
        // To tette grupper på hver sin side + ett fjernt punkt.
        let mut pts = Vec::new();
        for i in 0..5 {
            pts.push(vec![0.0 + i as f32 * 0.01, 0.0]);
        }
        for i in 0..5 {
            pts.push(vec![10.0 + i as f32 * 0.01, 10.0]);
        }
        pts.push(vec![50.0, -50.0]);
        let labels = run(&pts, 3);
        assert_eq!(labels, [0, 0, 0, 0, 0, 1, 1, 1, 1, 1, -1]);
    }

    #[test]
    fn feil_som_sklearn() {
        let p: Vec<f32> = vec![0.0, 1.0];
        assert_eq!(fit_predict(&[&p], 2), Err(HdbscanError::SingleSample));
        assert_eq!(
            fit_predict(&[&p, &p], 1),
            Err(HdbscanError::MinClusterSize(1))
        );
        assert_eq!(
            fit_predict(&[&p, &p], 3),
            Err(HdbscanError::MinSamples(3, 2))
        );
        assert_eq!(fit_predict(&[], 3), Ok(vec![]));
    }

    #[test]
    fn alt_stoy_naar_ingen_klynge_er_stor_nok() {
        let pts = vec![vec![0.0f32], vec![1.0], vec![5.0]];
        assert_eq!(run(&pts, 3), [-1, -1, -1]);
    }
}
