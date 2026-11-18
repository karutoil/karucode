#!/usr/bin/env node
/**
 * The web dependency gate: `npm audit` with the exceptions written down.
 *
 * CI runs this instead of a bare `npm audit --audit-level=moderate` because a
 * transitive advisory with no fix in existence would otherwise fail every commit
 * on `main` until an upstream release lands, and the usual reaction to that is to
 * weaken the whole check. Here the check stays at `moderate`: anything new fails,
 * and the one or two advisories we have looked at are listed in `ACCEPTED` with
 * the reason and the condition that retires them.
 *
 * Each entry is keyed by its GHSA id and has to say three things:
 *
 *   reason  why the advisory does not apply to us, or why we cannot remove it;
 *   scope   what can reach the vulnerable code (dev/build only, not shipped);
 *   until   the release or change that makes the entry unnecessary.
 *
 * Adding an entry is a deliberate act with a reviewable diff - the same shape as
 * `[advisories] ignore` in the repository's `deny.toml` for the Rust graph. An
 * entry that no longer matches any advisory is reported so it gets deleted.
 *
 * Usage:
 *   npm run audit                       # from web/
 *   node scripts/audit-gate.mjs         # same thing
 *   node scripts/audit-gate.mjs --json  # the raw npm report, for debugging
 */

import { execFileSync } from 'node:child_process';
import { fileURLToPath, pathToFileURL } from 'node:url';

/** Severities npm reports, in the order it uses for `--audit-level`. */
const SEVERITIES = ['info', 'low', 'moderate', 'high', 'critical'];

/** The floor CI has always used: `npm audit --audit-level=moderate`. */
export const THRESHOLD = 'moderate';

/**
 * Advisories we have reviewed and decided to live with.
 *
 * Keep this list short and keep every entry honest: if a release fixes one of
 * these, delete the entry in the same commit as the dependency update.
 */
export const ACCEPTED = new Map([
  [
    'GHSA-fc5q-39xc-vjpf',
    {
      package: 'picomatch',
      reason:
        'ReDoS through single-branch repeated extglobs. `picomatch` <= 4.0.7 is the whole published range ' +
        'and 4.0.7 is still `latest`, so there is nothing to upgrade to yet; npm offers only a downgrade of ' +
        '@preact/preset-vite and vitest, which would break the build.',
      scope:
        'Dev/build tooling only: it arrives through vite -> tinyglobby/fdir and vitest -> vite, plus ' +
        '@preact/preset-vite -> @rollup/pluginutils. None of it is in the bundle `web/dist` embeds, and the ' +
        'patterns it matches come from our own config, never from a request.',
      until: 'a `picomatch` release above 4.0.7 is installed (then delete this entry)',
    },
  ],
]);

/** Numeric rank for a severity name; unknown names rank as `info`. */
export function severityRank(severity) {
  const index = SEVERITIES.indexOf(String(severity ?? '').toLowerCase());
  return index === -1 ? 0 : index;
}

/**
 * A GHSA id in the one spelling used everywhere: `GHSA-fc5q-39xc-vjpf`.
 *
 * GitHub prints them in either case and `ACCEPTED` below has to match a report
 * from any npm, so the comparison is case-insensitive on the middle segments but
 * the prefix stays upper case, which is what a human reads in an advisory page.
 */
function normalizeAdvisoryId(value) {
  const match = /^GHSA-([0-9A-Za-z]{4}-[0-9A-Za-z]{4}-[0-9A-Za-z]{4})$/i.exec(String(value ?? '').trim());
  return match ? `GHSA-${match[1].toLowerCase()}` : '';
}

/** The GHSA id an advisory is known by, from npm's own field or its url. */
export function advisoryId(advisory) {
  const field = normalizeAdvisoryId(advisory?.github_advisory_id);
  if (field) return field;
  const fromUrl = /(GHSA-[0-9A-Za-z]{4}-[0-9A-Za-z]{4}-[0-9A-Za-z]{4})/i.exec(String(advisory?.url ?? ''));
  return fromUrl ? normalizeAdvisoryId(fromUrl[1]) : '';
}

/**
 * Every *leaf* advisory in an `npm audit --json` report, deduplicated.
 *
 * A report does not only list advisories: each entry in `vulnerabilities` names
 * the packages it affects *and* the advisories behind it, and a package can be
 * flagged only because something below it is. The objects in `via` are therefore
 * the real findings, and a `via` string is a pointer to another flagged package,
 * not an advisory of its own. Audit report v2 (npm 7+) has `vulnerabilities`;
 * v1 had `advisories`, still read for an older npm on a developer machine.
 */
export function collectAdvisories(report) {
  const found = new Map();
  const keep = (advisory) => {
    const existing = found.get(advisory.id);
    if (!existing || severityRank(advisory.severity) > severityRank(existing.severity)) {
      found.set(advisory.id, advisory);
    }
  };

  for (const [name, entry] of Object.entries(report?.vulnerabilities ?? {})) {
    for (const via of entry?.via ?? []) {
      // A string is "this package is affected because <package> is", not a finding.
      if (!via || typeof via !== 'object') continue;
      const id = advisoryId(via);
      if (!id) continue;
      keep({
        id,
        package: via.dependency ?? via.name ?? name,
        title: via.title ?? '',
        severity: via.severity ?? entry?.severity ?? 'moderate',
        url: via.url ?? '',
        range: via.range ?? '',
      });
    }
  }

  for (const entry of Object.values(report?.advisories ?? {})) {
    const id = advisoryId(entry);
    if (!id) continue;
    keep({
      id,
      package: entry.module_name ?? entry.name ?? '',
      title: entry.title ?? '',
      severity: entry.severity ?? 'moderate',
      url: entry.url ?? '',
      range: entry.vulnerable_versions ?? '',
    });
  }

  return [...found.values()];
}

/**
 * Split a report into what blocks the build and what was waived.
 *
 * `blocking` is every advisory at or above `threshold` with no `ACCEPTED` entry;
 * `waived` is every advisory that has one (whatever its severity, because it is
 * printed); `stale` is an accepted id nothing reported any more.
 */
export function evaluateAudit(report, accepted = ACCEPTED, threshold = THRESHOLD) {
  if (report?.error) {
    const detail = report.error.detail ?? report.error.summary ?? 'no detail';
    throw new Error(`npm audit could not run: ${detail}`);
  }

  const advisories = collectAdvisories(report);
  const counts = report?.metadata?.vulnerabilities;
  const reported =
    typeof counts === 'object' && counts !== null
      ? SEVERITIES.slice(severityRank(threshold)).reduce((total, level) => total + (counts[level] ?? 0), 0)
      : 0;
  if (reported > 0 && advisories.length === 0) {
    // Never pass on a report we could not read: a renamed field would otherwise
    // turn this gate into a rubber stamp.
    throw new Error(
      `npm audit reported ${reported} vulnerability/ies but the report listed no advisories; ` +
        'the report format changed and this gate no longer understands it',
    );
  }

  const floor = severityRank(threshold);
  const blocking = [];
  const waived = [];
  for (const advisory of advisories) {
    const waiver = accepted.get(advisory.id);
    if (waiver) waived.push({ ...advisory, waiver });
    else if (severityRank(advisory.severity) >= floor) blocking.push(advisory);
  }
  const seen = new Set(advisories.map((advisory) => advisory.id));
  const stale = [...accepted.entries()]
    .filter(([id]) => !seen.has(id))
    .map(([id, waiver]) => ({ id, package: waiver.package ?? '' }));

  return { blocking, waived, stale };
}

/** Render `evaluateAudit`'s result for a terminal (or a CI log). */
export function formatReport({ blocking, waived, stale }, threshold = THRESHOLD) {
  const lines = [`web dependency audit (npm audit, failing at ${threshold} and above)`];

  if (blocking.length > 0) {
    lines.push('', `blocking (${blocking.length}):`);
    for (const advisory of blocking) {
      lines.push(
        `  ${advisory.severity.padEnd(8)} ${advisory.package || '(unknown)'}` +
          `${advisory.range ? ` ${advisory.range}` : ''}  ${advisory.id}`,
        `      ${advisory.title || '(no title)'}`,
        ...(advisory.url ? [`      ${advisory.url}`] : []),
      );
    }
    lines.push(
      '',
      'Upgrade the dependency (`npm update <pkg>`, or bump the parent that pulls it in).',
      'If no release fixes it, add the advisory to ACCEPTED in web/scripts/audit-gate.mjs',
      'with the reason, the scope that can reach it, and the condition that removes it.',
    );
  }

  if (waived.length > 0) {
    lines.push('', `accepted (${waived.length}):`);
    for (const advisory of waived) {
      lines.push(
        `  ${advisory.severity.padEnd(8)} ${advisory.package || '(unknown)'}  ${advisory.id}  ${advisory.title}`,
        `      reason: ${advisory.waiver.reason}`,
        `      scope:  ${advisory.waiver.scope}`,
        `      until:  ${advisory.waiver.until}`,
      );
    }
  }

  if (stale.length > 0) {
    lines.push('', `note: ${stale.length} accepted advisor${stale.length === 1 ? 'y' : 'ies'} no longer reported:`);
    for (const entry of stale) {
      lines.push(`  ${entry.id} (${entry.package || '?'}) - delete this entry from ACCEPTED in web/scripts/audit-gate.mjs`);
    }
  }

  const plural = (count, word) => `${count} ${word}${count === 1 ? 'y' : 'ies'}`;
  lines.push(
    '',
    `${plural(blocking.length, 'blocking advisor')}, ${waived.length} accepted.`,
  );
  return lines.join('\n');
}

/** Run `npm audit --json` in `cwd` and return its parsed report. */
function readAuditReport(cwd) {
  let stdout;
  try {
    stdout = execFileSync('npm', ['audit', '--json'], {
      cwd,
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'pipe'],
    });
  } catch (error) {
    // Exit status 1 is how `npm audit` says "vulnerabilities found"; the JSON is
    // still on stdout. Anything else, or no JSON at all, is a real failure.
    const captured = typeof error?.stdout === 'string' ? error.stdout.trim() : '';
    if (captured.startsWith('{')) stdout = captured;
    else {
      const stderr = String(error?.stderr ?? '').trim();
      throw new Error(`npm audit did not run: ${stderr || error?.message || 'no output'}`);
    }
  }
  try {
    return JSON.parse(stdout);
  } catch {
    throw new Error(`npm audit printed something that is not JSON:\n${stdout?.slice(0, 2000)}`);
  }
}

function main(argv) {
  const cwd = fileURLToPath(new URL('../', import.meta.url));

  let report;
  try {
    report = readAuditReport(cwd);
  } catch (error) {
    process.stderr.write(`audit-gate: ${error.message}\n`);
    return 2;
  }

  if (argv.includes('--json')) {
    process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
    return 0;
  }

  let result;
  try {
    result = evaluateAudit(report);
  } catch (error) {
    process.stderr.write(`audit-gate: ${error.message}\n`);
    return 2;
  }

  process.stdout.write(`${formatReport(result)}\n`);
  return result.blocking.length === 0 ? 0 : 1;
}

if (process.argv[1] && pathToFileURL(process.argv[1]).href === import.meta.url) {
  process.exitCode = main(process.argv.slice(2));
}
