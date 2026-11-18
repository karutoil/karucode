import { describe, expect, it } from 'vitest';
import {
  ACCEPTED,
  advisoryId,
  collectAdvisories,
  evaluateAudit,
  formatReport,
  severityRank,
  THRESHOLD,
} from './audit-gate.mjs';

function advisory(overrides = {}) {
  return {
    source: 1,
    name: 'picomatch',
    dependency: 'picomatch',
    title: 'picomatch vulnerable to ReDoS through single-branch repeated extglobs',
    url: 'https://github.com/advisories/GHSA-fc5q-39xc-vjpf',
    severity: 'high',
    range: '<=4.0.7',
    ...overrides,
  };
}

/** An `npm audit --json` v2 report, shaped like npm 7+ writes it. */
function report(vulnerabilities, counts = {}) {
  return {
    auditReportVersion: 2,
    vulnerabilities,
    metadata: {
      vulnerabilities: { info: 0, low: 0, moderate: 0, high: 0, critical: 0, ...counts },
    },
  };
}

/** One flagged package, with `via` as npm writes it. */
function flaggedVia(name, via, severity = 'high') {
  return report(
    {
      [name]: { name, severity, isDirect: false, via, effects: [], range: '*', nodes: [], fixAvailable: false },
    },
    { [severity]: 1 },
  );
}

describe('severityRank', () => {
  it('orders the npm severities and treats anything unknown as info', () => {
    expect(severityRank('info')).toBeLessThan(severityRank('low'));
    expect(severityRank('low')).toBeLessThan(severityRank('moderate'));
    expect(severityRank('moderate')).toBeLessThan(severityRank('high'));
    expect(severityRank('high')).toBeLessThan(severityRank('critical'));
    expect(severityRank(undefined)).toBe(severityRank('info'));
    expect(severityRank('nonsense')).toBe(severityRank('info'));
  });
});

describe('advisoryId', () => {
  it('reads the id from the advisory url', () => {
    expect(advisoryId(advisory())).toBe('GHSA-fc5q-39xc-vjpf');
  });

  it('prefers npm\u2019s own field and compares ids in one case', () => {
    expect(advisoryId({ github_advisory_id: 'GHSA-ABCD-EFGH-IJKL' })).toBe('GHSA-abcd-efgh-ijkl');
    expect(advisoryId({ url: 'https://github.com/advisories/GHSA-FC5Q-39XC-VJPF' })).toBe(
      'GHSA-fc5q-39xc-vjpf',
    );
  });

  it('returns an empty string when the entry names no advisory', () => {
    expect(advisoryId({ url: 'https://github.com/advisories/CVE-2024-1' })).toBe('');
    expect(advisoryId(undefined)).toBe('');
  });
});

describe('collectAdvisories', () => {
  it('collects the leaf advisories and ignores via strings', () => {
    const collected = collectAdvisories(
      flaggedVia('vite', [advisory(), 'tinyglobby'], 'high'),
    );
    expect(collected).toHaveLength(1);
    expect(collected[0]).toMatchObject({
      id: 'GHSA-fc5q-39xc-vjpf',
      package: 'picomatch',
      severity: 'high',
    });
  });

  it('deduplicates one advisory reached through several packages', () => {
    const collected = collectAdvisories({
      ...report({
        vite: {
          name: 'vite',
          severity: 'high',
          via: [advisory(), 'tinyglobby'],
          effects: [],
          range: '*',
          nodes: [],
          fixAvailable: false,
        },
        tinyglobby: {
          name: 'tinyglobby',
          severity: 'high',
          via: [advisory()],
          effects: ['vite'],
          range: '*',
          nodes: [],
          fixAvailable: false,
        },
      }),
    });
    expect(collected.map((entry) => entry.id)).toEqual(['GHSA-fc5q-39xc-vjpf']);
  });

  it('keeps the highest severity seen for one advisory', () => {
    const collected = collectAdvisories({
      ...report({
        picomatch: {
          name: 'picomatch',
          severity: 'critical',
          via: [advisory({ severity: 'moderate' }), advisory({ severity: 'critical', url: advisory().url })],
          effects: [],
          range: '*',
          nodes: [],
          fixAvailable: false,
        },
      }),
    });
    expect(collected).toHaveLength(1);
    expect(collected[0].severity).toBe('critical');
  });

  it('reads the v1 `advisories` shape an older npm writes', () => {
    const collected = collectAdvisories({
      advisories: {
        1247710: {
          id: 1247710,
          module_name: 'picomatch',
          github_advisory_id: 'GHSA-fc5q-39xc-vjpf',
          title: 'picomatch vulnerable to ReDoS',
          severity: 'high',
          url: 'https://github.com/advisories/GHSA-fc5q-39xc-vjpf',
          vulnerable_versions: '<=4.0.7',
        },
      },
      metadata: { vulnerabilities: { info: 0, low: 0, moderate: 0, high: 1, critical: 0 } },
    });
    expect(collected).toEqual([
      {
        id: 'GHSA-fc5q-39xc-vjpf',
        package: 'picomatch',
        title: 'picomatch vulnerable to ReDoS',
        severity: 'high',
        url: 'https://github.com/advisories/GHSA-fc5q-39xc-vjpf',
        range: '<=4.0.7',
      },
    ]);
  });
});

describe('evaluateAudit', () => {
  const accepted = new Map([
    [
      'GHSA-fc5q-39xc-vjpf',
      { package: 'picomatch', reason: 'dev only', scope: 'build tooling', until: '4.0.8' },
    ],
  ]);

  it('waives an advisory that has an entry and says why', () => {
    const result = evaluateAudit(flaggedVia('vite', [advisory()]), accepted);
    expect(result.blocking).toEqual([]);
    expect(result.waived).toHaveLength(1);
    expect(result.waived[0].waiver.reason).toBe('dev only');
    expect(result.stale).toEqual([]);
  });

  it('blocks an advisory nobody has reviewed', () => {
    const result = evaluateAudit(flaggedVia('vite', [advisory({ url: 'https://github.com/advisories/GHSA-aaaa-bbbb-cccc' })]), accepted);
    expect(result.blocking.map((entry) => entry.id)).toEqual(['GHSA-aaaa-bbbb-cccc']);
    expect(result.waived).toEqual([]);
  });

  it('fails on a report npm refused to produce', () => {
    expect(() =>
      evaluateAudit({ error: { code: 'ENOLOCK', summary: 'This command requires an existing lockfile.' } }, accepted),
    ).toThrow(/ENOLOCK|lockfile/);
  });

  it('fails rather than passes when the report shape changes', () => {
    // metadata counts a vulnerability, but nothing parses out of the report.
    expect(() =>
      evaluateAudit(report({ picomatch: { name: 'picomatch', severity: 'high', via: ['vite'] } }, { high: 1 }), accepted),
    ).toThrow(/listed no advisories/);
  });

  it('reports an entry that no advisory uses any more', () => {
    const result = evaluateAudit(report({}, {}), accepted);
    expect(result.stale).toEqual([{ id: 'GHSA-fc5q-39xc-vjpf', package: 'picomatch' }]);
    expect(result.blocking).toEqual([]);
  });

  it('ignores advisories below the threshold but still names waived ones', () => {
    const low = advisory({ url: 'https://github.com/advisories/GHSA-dddd-eeee-ffff', severity: 'low' });
    expect(evaluateAudit(flaggedVia('some-pkg', [low], 'low'), new Map()).blocking).toEqual([]);
    expect(evaluateAudit(flaggedVia('some-pkg', [low], 'low'), accepted).waived).toHaveLength(0);
  });

  it('passes a clean report', () => {
    const result = evaluateAudit(report({}, {}), new Map());
    expect(result).toEqual({ blocking: [], waived: [], stale: [] });
  });

  it('ships a gate that is at moderate by default', () => {
    expect(THRESHOLD).toBe('moderate');
    // Whatever is accepted today must still carry the three explanations.
    for (const [id, waiver] of ACCEPTED) {
      expect(id).toMatch(/^GHSA-/);
      expect(waiver.reason.length).toBeGreaterThan(40);
      expect(waiver.scope.length).toBeGreaterThan(20);
      expect(waiver.until.length).toBeGreaterThan(10);
    }
  });
});

describe('formatReport', () => {
  it('names each blocking advisory and how to waive it', () => {
    const text = formatReport(
      evaluateAudit(flaggedVia('esbuild', [advisory({ url: 'https://github.com/advisories/GHSA-aaaa-bbbb-cccc', name: 'esbuild', dependency: 'esbuild' })]), new Map()),
    );
    expect(text).toContain('blocking (1)');
    expect(text).toContain('GHSA-aaaa-bbbb-cccc');
    expect(text).toContain('https://github.com/advisories/GHSA-aaaa-bbbb-cccc');
    expect(text).toContain('ACCEPTED in web/scripts/audit-gate.mjs');
  });

  it('prints the waiver and the stale note', () => {
    const accepted = new Map([
      ['GHSA-fc5q-39xc-vjpf', { package: 'picomatch', reason: 'dev only', scope: 'build tooling', until: '4.0.8' }],
      ['GHSA-zzzz-yyyy-xxxx', { package: 'gone', reason: 'gone', scope: 'gone', until: 'never' }],
    ]);
    const text = formatReport(evaluateAudit(flaggedVia('vite', [advisory()]), accepted));
    expect(text).toContain('accepted (1)');
    expect(text).toContain('reason: dev only');
    expect(text).toContain('until:  4.0.8');
    expect(text).toContain('GHSA-zzzz-yyyy-xxxx');
    expect(text).toContain('delete this entry from ACCEPTED in web/scripts/audit-gate.mjs');
  });
});
