
CREATE TABLE IF NOT EXISTS tagging_profiles (       -- [v5] navngitt, gjenbrukbar tagging-profil
  id              INTEGER PRIMARY KEY,
  name            TEXT NOT NULL UNIQUE,             -- "Design & håndverk"
  focus           TEXT,                             -- fokus-direktivteksten (kan være tom)
  categories      TEXT NOT NULL DEFAULT '[]',       -- JSON ordnet liste: ["materiale","teknikk",...]
  created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS sources (
  id              INTEGER PRIMARY KEY,
  root_path       TEXT NOT NULL UNIQUE,
  source_type     TEXT NOT NULL DEFAULT 'local',   -- 'local' | 'dropbox'
  added_at        TEXT NOT NULL DEFAULT (datetime('now')),
  last_scanned_at TEXT,                             -- NULL = aldri scannet
  file_count      INTEGER NOT NULL DEFAULT 0,
  focus           TEXT,                             -- [v4] vestigial — avløst av profile_id (v5), beholdt for migrasjon
  profile_id      INTEGER REFERENCES tagging_profiles(id) ON DELETE SET NULL  -- [v5] NULL = arv globalt default_profile_id
);

CREATE TABLE IF NOT EXISTS scan_index (             -- rask forhåndssjekk, unngår re-hashing
  path            TEXT PRIMARY KEY,
  size            INTEGER NOT NULL,
  mtime           REAL NOT NULL,
  content_hash    TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS settings (               -- bruker-konfig, skrives fra GUI
  key             TEXT PRIMARY KEY,                 -- f.eks. 'openrouter_api_key', 'ai_model'
  value           TEXT
);

CREATE TABLE IF NOT EXISTS images (
  id              INTEGER PRIMARY KEY,
  content_hash    TEXT NOT NULL UNIQUE,             -- SHA-256 hex
  owner_id        TEXT NOT NULL DEFAULT 'bjorn',
  width           INTEGER,
  height          INTEGER,
  taken_at        TEXT,                             -- ISO8601 fra EXIF DateTimeOriginal
  year            INTEGER,                          -- avledet fra taken_at
  season          TEXT,                             -- 'vinter'|'vår'|'sommer'|'høst'
  camera_make     TEXT,
  camera_model    TEXT,
  gps_lat         REAL,
  gps_lon         REAL,
  place_name      TEXT,                             -- Kartverket-geocodet
  excluded        INTEGER NOT NULL DEFAULT 0,       -- [v2] 1 = skjult fra vyer (mykt, reversibelt)
  created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS files (                  -- alle fysiske kopier av et bilde
  id              INTEGER PRIMARY KEY,
  image_id        INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
  source_id       INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
  path            TEXT NOT NULL UNIQUE,
  last_seen_at    TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS derived (                -- nedskalerte arbeidskopier
  id              INTEGER PRIMARY KEY,
  image_id        INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
  size            TEXT NOT NULL,                    -- '1024' | '300'
  path            TEXT NOT NULL,
  UNIQUE(image_id, size)
);

CREATE TABLE IF NOT EXISTS processing_state (       -- idempotens per steg
  image_id        INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
  stage           TEXT NOT NULL,                    -- 'derive'|'ai'|'faces'
  status          TEXT NOT NULL DEFAULT 'pending',  -- 'pending'|'done'|'error'
  error           TEXT,
  updated_at      TEXT NOT NULL DEFAULT (datetime('now')),
  PRIMARY KEY (image_id, stage)
);

-- Defineres her, fylles av andre TB-er:
CREATE TABLE IF NOT EXISTS ai_tags (
  id              INTEGER PRIMARY KEY,
  image_id        INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
  provider        TEXT NOT NULL,
  scene           TEXT,
  event_type      TEXT,
  season_ai       TEXT,
  indoor_outdoor  TEXT,
  person_count    INTEGER,
  description     TEXT,
  raw_json        TEXT,
  cost            REAL,                             -- [v3] faktisk USD-kost fra OpenRouter usage.cost (NULL = ukjent)
  tagged_at       TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS tags (
  id              INTEGER PRIMARY KEY,
  name            TEXT NOT NULL UNIQUE,
  category        TEXT                              -- [v5] kategori fra tagging-profil; NULL = ukategorisert/generisk
);
CREATE TABLE IF NOT EXISTS image_tags (
  image_id        INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
  tag_id          INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
  PRIMARY KEY (image_id, tag_id)
);

CREATE TABLE IF NOT EXISTS persons (
  cluster_id      INTEGER PRIMARY KEY,
  name            TEXT                              -- NULL = ikke navngitt
);
CREATE TABLE IF NOT EXISTS faces (
  id              INTEGER PRIMARY KEY,
  image_id        INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
  bbox            TEXT NOT NULL,                    -- JSON [x,y,w,h] i originalkoordinater
  embedding       BLOB NOT NULL,                   -- float32-vektor
  cluster_id      INTEGER REFERENCES persons(cluster_id) ON DELETE SET NULL,
  locked          INTEGER NOT NULL DEFAULT 0        -- [v2] 1 = frosset/ignorert; cluster() rører det aldri
);

CREATE TABLE IF NOT EXISTS albums (
  id              INTEGER PRIMARY KEY,
  name            TEXT NOT NULL,
  created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE TABLE IF NOT EXISTS album_items (
  album_id        INTEGER NOT NULL REFERENCES albums(id) ON DELETE CASCADE,
  image_id        INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
  position        INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (album_id, image_id)
);

-- Indekser
CREATE INDEX IF NOT EXISTS idx_images_taken_at   ON images(taken_at);
CREATE INDEX IF NOT EXISTS idx_images_year       ON images(year);
CREATE INDEX IF NOT EXISTS idx_files_image       ON files(image_id);
CREATE INDEX IF NOT EXISTS idx_procstate_lookup  ON processing_state(stage, status);
CREATE INDEX IF NOT EXISTS idx_faces_cluster     ON faces(cluster_id);
CREATE INDEX IF NOT EXISTS idx_aitags_image      ON ai_tags(image_id);
CREATE INDEX IF NOT EXISTS idx_images_excluded   ON images(excluded);   -- [v2]
