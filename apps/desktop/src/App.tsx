import "./App.css";

/// The window, and nothing behind it yet.
///
/// The three panes, the transcript and the checkpoints arrive with M3-T06 and
/// M3-T07; the commands they call arrive with M3-T04. Until then this says so
/// rather than pretending otherwise.
function App() {
  return (
    <main className="shell">
      <h1>Eavery</h1>
      <p className="lead">
        An agent for everyday work on your own files. Every change is
        checkpointed before it happens, and one button takes it back.
      </p>
      <p className="note">
        Nothing is wired up yet. The core runs from the terminal today:
        <code>eavery-cli project open &lt;folder&gt;</code>, then
        <code>eavery-cli run</code>.
      </p>
    </main>
  );
}

export default App;
