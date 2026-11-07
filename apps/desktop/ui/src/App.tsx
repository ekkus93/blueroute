// Bootstrap shell only (P12-001). It deliberately has no daemon access, no
// network actions and no Internet-sharing UI; later P12 tasks add those through
// narrow, typed Tauri commands.
export function App() {
  return (
    <main>
      <h1>BlueRoute</h1>
      <p>Connect nearby Linux computers over Bluetooth.</p>
    </main>
  );
}
