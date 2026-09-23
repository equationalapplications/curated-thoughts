import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const { lintSeededTiers, typeUntypedFacts, useWikiStatus } = vi.hoisted(() => ({
  lintSeededTiers: vi.fn(),
  typeUntypedFacts: vi.fn(),
  useWikiStatus: vi.fn(() => ({ busy: false })),
}));
vi.mock('../../../lib/wiki', () => ({ lintSeededTiers, typeUntypedFacts }));
vi.mock('../../../hooks/useWikiStatus', () => ({ useWikiStatus }));

import { HealthReportPanel } from '../HealthReportPanel';

const report = {
  danglingEdges: 2, manifestViolations: 1, untypedFacts: 9, drafts: 3, unverifiedInferred: 4,
  sample: { danglingEdgeIds: ['e1', 'e2'], manifestViolationEdgeIds: ['e9'] },
};

describe('HealthReportPanel', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useWikiStatus.mockReturnValue({ busy: false });
    lintSeededTiers.mockResolvedValue([{ entityId: 'tier_fact', report }]);
    typeUntypedFacts.mockResolvedValue({ typed: 6, remaining: 3 });
  });

  it('runs lint on demand and shows counts and samples', async () => {
    render(<HealthReportPanel />);
    expect(lintSeededTiers).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: 'Run health report' }));
    expect(await screen.findByText('tier_fact')).toBeInTheDocument();
    // Anchored so a future '19' or '90' won't match by accident.
    expect(screen.getByText('Untyped facts').nextSibling).toHaveTextContent(/^9$/);
    expect(screen.getByText(/e1, e2/)).toBeInTheDocument();
  });

  it('types untyped facts and reports the outcome', async () => {
    render(<HealthReportPanel />);
    await userEvent.click(screen.getByRole('button', { name: 'Type untyped facts' }));
    expect(typeUntypedFacts).toHaveBeenCalledTimes(1);
    expect(await screen.findByText('Typed 6 facts; 3 still untyped.')).toBeInTheDocument();
  });

  it('disables typing while a wiki job is busy', () => {
    useWikiStatus.mockReturnValue({ busy: true });
    render(<HealthReportPanel />);
    expect(screen.getByRole('button', { name: 'Type untyped facts' })).toBeDisabled();
  });
});
