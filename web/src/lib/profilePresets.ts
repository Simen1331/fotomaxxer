// Start-profiler — rene frontend-maler som forhåndsfyller skjemaet når du
// oppretter en profil. Engine kjenner dem ikke; du finjusterer fritt etterpå.
// Kategoriene styrer både prompten (modellen bøtter stikkord per kategori) og
// gruppering i filter-sidebaren.
export interface ProfilePreset {
  name: string;
  focus: string;
  categories: string[];
}

export const PROFILE_PRESETS: ProfilePreset[] = [
  {
    name: "Design & håndverk",
    focus: "møbler, trearbeid, treskjæring, kunst og design",
    categories: ["materiale", "teknikk", "stil & form", "motiv"],
  },
  {
    name: "Tom profil",
    focus: "",
    categories: [],
  },
];
