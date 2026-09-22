import { useState } from 'react';
import type { WikiLintReport } from '@equationalapplications/react-llm-wiki';
import { lintSeededTiers, typeUntypedFacts } from '../../lib/wiki';
import { useWikiStatus } from '../../hooks/useWikiStatus';

const ROWS: Array<[keyof Omit<WikiLintReport, 'sample'>, string]> = [
  ['danglingEdges', 'Dangling edges'],
  ['manifestViolations', 'Manifest violations'],
  ['untypedFacts', 'Untyped facts'],
  ['drafts', 'Drafts'],
  ['unverifiedInferred', 'Unverified inferred facts'],
];

/**
 * Read-only engine health report (spec CT-REQ-LINT-01) plus the classifier
 * use site, "Type untyped facts" (spec §5.3 rev 2). No repair buttons: edge
 * repair stays with `purge_off_manifest_edges_cmd`.
 */
export function HealthReportPanel() {
  const { busy } = useWikiStatus();
  const [reports, setReports] = useState<Array<{ entityId: string; report: WikiLintReport }> | null>(null);
  const [running, setRunning] = useState<'lint' | 'type' | null>(null);
  const [typingResult, setTypingResult] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function runLint() {
    setRunning('lint');
    setError(null);
    try {
      setReports(await lintSeededTiers());
    } catch (err) {
      setError(String(err));
    } finally {
      setRunning(null);
    }
  }

  async function runTyping() {
    setRunning('type');
    setError(null);
    setTypingResult(null);
    try {
      const { typed, remaining } = await typeUntypedFacts();
      setTypingResult(`Typed ${typed} facts; ${remaining} still untyped.`);
    } catch (err) {
      setError(String(err));
    } finally {
      setRunning(null);
    }
  }

  return (
    <section className="maintenance-health" aria-labelledby="health-heading">
      <h4 id="health-heading">Health report</h4>
      {error && (
        <p className="maintenance-error" role="alert">
          Health report: {error}
        </p>
      )}
      <button type="button" disabled={running !== null} onClick={() => void runLint()}>
        Run health report
      </button>
      <button type="button" disabled={busy || running !== null} onClick={() => void runTyping()}>
        Type untyped facts
      </button>
      <p className="maintenance-description">
        Assigns ontology types to facts that have none. Uses the classifier when one is configured
        in Settings → Models; otherwise uses the generation model.
      </p>
      {typingResult && <p className="maintenance-description">{typingResult}</p>}
      {reports?.map(({ entityId, report }) => (
        <div key={entityId} className="maintenance-health-tier">
          <h5>{entityId}</h5>
          <dl>
            {ROWS.map(([key, label]) => (
              <div key={key}>
                <dt>{label}</dt>
                <dd>{report[key]}</dd>
              </div>
            ))}
          </dl>
          {report.sample.danglingEdgeIds.length > 0 && (
            <p className="maintenance-description">
              Dangling edge sample: {report.sample.danglingEdgeIds.join(', ')}
            </p>
          )}
          {report.sample.manifestViolationEdgeIds.length > 0 && (
            <p className="maintenance-description">
              Violation sample: {report.sample.manifestViolationEdgeIds.join(', ')}
            </p>
          )}
        </div>
      ))}
    </section>
  );
}
