import TitleBar from "./components/TitleBar";
import DiskList from "./components/DiskList";
import DiskDetail from "./components/DiskDetail";
import { readDiskRoute } from "./diskRoute";
import { Router, useLocation } from "./router";

const CurrentPage = () => {
  const { pathname } = useLocation();
  return pathname === "/disk" ? <DiskDetail /> : <DiskList />;
};

function App() {
  const diskRoute = readDiskRoute();
  const initialLocation = diskRoute
    ? { pathname: "/disk", state: diskRoute }
    : { pathname: "/" };

  return (
    <Router initialLocation={initialLocation}>
      <div className="app-shell">
        <TitleBar></TitleBar>
        <CurrentPage />
      </div>
    </Router>
  );
}

export default App;
