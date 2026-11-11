import { beginWindowDrag, toggleMaximize } from "../windowChrome";
import { WindowControls } from "./WindowControls";

/** Keep window management available before an account is connected, too. */
export function StandaloneTitlebar() {
  return (
    <header
      className="standalone-titlebar"
      onMouseDown={beginWindowDrag}
      onDoubleClick={toggleMaximize}
    >
      <WindowControls />
    </header>
  );
}
