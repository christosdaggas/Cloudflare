import { Dialog } from "../components/Dialog";
import { Badge } from "../components/ui";
import { useApp } from "../store";

export function WhatsNewDialog({ onClose }: { onClose: () => void }) {
  const bootstrap = useApp((store) => store.bootstrap);
  const releases = bootstrap?.releases ?? [];

  return (
    <Dialog
      title="What's New"
      description="Release notes for this build and the ones before it."
      onClose={onClose}
      footer={
        <button className="btn btn-primary" onClick={onClose}>
          Close
        </button>
      }
    >
      <div className="stack">
        {releases.map((release) => (
          <div className="dialog-section" key={release.version}>
            <div className="row-inline spread">
              <div className="row-inline">
                <span style={{ fontWeight: 700 }}>Version {release.version}</span>
                <Badge tone="accent">{release.date}</Badge>
              </div>
            </div>
            <div className="steps">
              {release.changes.map((change) => (
                <div className="step" key={change}>
                  <div className="step-marker">•</div>
                  <div className="step-body">
                    <div className="step-title" style={{ fontWeight: 480 }}>
                      {change}
                    </div>
                  </div>
                </div>
              ))}
            </div>
          </div>
        ))}
        {releases.length === 0 && (
          <div className="field-hint">No release notes were compiled into this build.</div>
        )}
      </div>
    </Dialog>
  );
}
