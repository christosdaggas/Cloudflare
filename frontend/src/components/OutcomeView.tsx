import { Dialog } from "./Dialog";
import { Badge, Notice } from "./ui";
import { SymbolicIcon } from "../icons";
import {
  dnsState,
  recoveryExplanation,
  recoveryLabel,
  stepLabel,
  stepStatus,
} from "../status";
import type { Outcome, StepResult } from "../types";

/** The step-by-step record of a publish, repair, disable or unpublish. */
export function OutcomeSteps({ steps }: { steps: StepResult[] }) {
  if (steps.length === 0) {
    return <div className="field-hint">Nothing was attempted.</div>;
  }

  return (
    <div className="steps">
      {steps.map((step, index) => {
        const status = stepStatus(step.status);
        return (
          <div className={`step ${step.status === "failed" ? "current" : "done"}`} key={`${step.step}-${index}`}>
            <div className="step-marker">
              <SymbolicIcon name={status.icon} size={13} />
            </div>
            <div className="step-body">
              <div className="step-title">{stepLabel(step.step)}</div>
              <div className="step-detail">{step.detail}</div>
            </div>
            <Badge tone={status.tone}>{status.label}</Badge>
          </div>
        );
      })}
    </div>
  );
}

export function OutcomeDialog({
  title,
  outcome,
  onClose,
  onRepairDns,
  onRollback,
  onOpenSettings,
  onOpenDashboard,
  busy = false,
}: {
  title: string;
  outcome: Outcome;
  onClose: () => void;
  onRepairDns?: () => void;
  onRollback?: () => void;
  onOpenSettings?: () => void;
  onOpenDashboard?: () => void;
  busy?: boolean;
}) {
  const dns = dnsState(outcome.dns);
  const recovery = outcome.recovery;

  let action: { label: string; run: () => void } | null = null;
  if (recovery?.kind === "repair_dns" && onRepairDns) {
    action = { label: recoveryLabel(recovery), run: onRepairDns };
  } else if (recovery?.kind === "roll_back_ingress" && onRollback) {
    action = { label: recoveryLabel(recovery), run: onRollback };
  } else if (recovery?.kind === "fix_permissions" && onOpenSettings) {
    action = { label: recoveryLabel(recovery), run: onOpenSettings };
  } else if (recovery?.kind === "resolve_conflict_by_hand" && onOpenDashboard) {
    action = { label: recoveryLabel(recovery), run: onOpenDashboard };
  }

  return (
    <Dialog
      title={title}
      description={outcome.hostname}
      onClose={onClose}
      wide
      footer={
        <>
          <div className="spacer" />
          {action && (
            <button type="button" className="btn" onClick={action.run} disabled={busy}>
              {action.label}
            </button>
          )}
          <button type="button" className="btn btn-primary" onClick={onClose} disabled={busy}>
            Close
          </button>
        </>
      }
    >
      <div className="stack">
        <div className="row-inline spread">
          <span className="field-label">DNS</span>
          <Badge tone={dns.tone} icon={dns.icon}>
            {dns.label}
          </Badge>
        </div>

        <OutcomeSteps steps={outcome.steps} />

        {recovery && (
          <Notice icon="dialog-warning-symbolic">
            <strong>{recoveryLabel(recovery)}.</strong> {recoveryExplanation(recovery)}
          </Notice>
        )}
      </div>
    </Dialog>
  );
}
