// Flytt ett element i en array fra én indeks til en annen (rent, uten mutasjon).
export function arrayMove<T>(arr: T[], from: number, to: number): T[] {
  const copy = arr.slice();
  const [item] = copy.splice(from, 1);
  copy.splice(to, 0, item);
  return copy;
}
