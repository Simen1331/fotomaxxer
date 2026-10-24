import { NavLink, Route, Routes } from "react-router-dom";
import Library from "./views/Library";
import Albums from "./views/Albums";
import AlbumDetail from "./views/AlbumDetail";
import People from "./views/People";
import PersonDetail from "./views/PersonDetail";
import Sources from "./views/Sources";
import Settings from "./views/Settings";
import SearchBar from "./components/SearchBar";

// S5: full router — Bibliotek + Personer + Album + Kilder + Innstillinger.
export default function App() {
  return (
    <div className="app">
      <header className="topbar">
        <div className="brand">
          <b>
            Foto<em>Maxxer</em>
          </b>
          <span>Arkiv</span>
        </div>
        <SearchBar />
        <nav className="nav">
          <NavLink to="/" end className={({ isActive }) => (isActive ? "on" : "")}>
            Bibliotek
          </NavLink>
          <NavLink to="/people" className={({ isActive }) => (isActive ? "on" : "")}>
            Personer
          </NavLink>
          <NavLink to="/albums" className={({ isActive }) => (isActive ? "on" : "")}>
            Album
          </NavLink>
          <NavLink to="/sources" className={({ isActive }) => (isActive ? "on" : "")}>
            Kilder
          </NavLink>
          <NavLink to="/settings" className={({ isActive }) => (isActive ? "on" : "")}>
            Innstillinger
          </NavLink>
        </nav>
      </header>
      <Routes>
        <Route path="/" element={<Library />} />
        <Route path="/people" element={<People />} />
        <Route path="/people/:clusterId" element={<PersonDetail />} />
        <Route path="/albums" element={<Albums />} />
        <Route path="/albums/:id" element={<AlbumDetail />} />
        <Route path="/sources" element={<Sources />} />
        <Route path="/settings" element={<Settings />} />
      </Routes>
    </div>
  );
}
