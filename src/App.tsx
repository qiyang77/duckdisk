import { MemoryRouter as Router, Route, Routes } from "react-router-dom";

import TitleBar from "./components/TitleBar";
import DiskList from "./components/DiskList";
import DiskDetail from "./components/DiskDetail";

function App() {
  return (
    <Router>
      <div className="flex h-full flex-col items-stretch justify-items-stretch overflow-hidden">
        <TitleBar></TitleBar>
        <Routes>
          <Route path="/" element={<DiskList />} />
          <Route path="/disk" element={<DiskDetail />} />
        </Routes>
      </div>
    </Router>
  );
}

export default App;
