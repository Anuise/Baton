# Validation record

Validated on 2026-09-24 with Windows/PowerShell, Node.js 24.19.0, Python 3.12.10, PyYAML 6.0.3, and `skills` 1.7.0. This repository contains instruction documents, not an executable scheduler; distinguish installation/static checks, contract exercises, and live agent execution.

## Repository and CLI inspection

The initial repository contained README.md and an MIT license on `main`, tracking `https://github.com/Anuise/issue-orchestrator.git`. No package metadata, generated catalog, contribution instructions, or prior PR workflow existed. No unrelated local changes were present. The installed stable `implement` skill was read for its testing/review/commit discipline; no upstream skill was modified or used as the orchestration implementation.

Executed:

```sh
npx --yes skills@latest --version
npx --yes skills@latest --help
npx --yes skills@latest add --help
npx --yes skills@latest update --help
```

The three help invocations returned the general CLI help, including add/list/find/update options. Help confirms project scope by default, `--global`, `--skill <skills>`, `--agent <agents>`, `--copy`, `--list`, and scoped updates. Agent identifiers `claude-code` and `codex` were additionally verified by successful installations; they were not inferred from product names. No `--skill=<name>` syntax is used in this repository.

## Local installation checks

From the source repository:

```sh
npx --yes skills@latest add . --list
```

Result: exactly one discoverable skill, `issue-orchestrator`, at the nested `skills/engineering/issue-orchestrator` location, without extra discovery flags.

Created two separate temporary project directories outside the source repository. In each, ran the following commands, substituting the actual absolute source repository path for `<source>`:

```sh
npx --yes skills@latest add <source> --skill issue-orchestrator --agent claude-code --copy --yes
npx --yes skills@latest list --agent claude-code --json

npx --yes skills@latest add <source> --skill issue-orchestrator --agent codex --copy --yes
npx --yes skills@latest list --agent codex --json
```

Both installed successfully. The returned project locations were `.claude/skills/issue-orchestrator` for Claude Code and `.agents/skills/issue-orchestrator` for Codex. Compared all five installed files byte-for-byte with the source skill; validated the installed YAML frontmatter and resolved every relative Markdown reference. All passed. This explicitly tests copy mode; it does not claim an independent symlink-mode test.

Global installation and update options are documented from actual CLI help. A named project update was also invoked in the temporary Codex project; the CLI reported no matching updatable installed skill for this local-source installation, so that is not evidence of a successful remote update. No global install/update was performed. Nothing in the user's real global agent configuration was changed for validation.

## Static and contract validation

The skill-creator `quick_validate.py` accepted the YAML frontmatter, name, description, and finished instructions. A separate Python/PyYAML check parsed installed frontmatter, checked full directory contents, and resolved local links. Final repository checks also inspect whitespace and scope.

An independent read-only agent exercised these scenarios against the actual documents. These are decision traces, not automated scheduler tests:

| Scenario | Observed contract decision |
| --- | --- |
| Three independent pending issues | Select one using risk then numeric/path tie-breaks; persist its attempt, verify, reread, repeat with a fresh worker |
| Issue 03 requires issue 01 | Keep 03 outside the ready frontier until 01's prerequisite acceptance evidence is verified |
| Worker discovers a prerequisite blocker | Persist blocker/release condition; preserve partial work; continue a different issue only when safe |
| Worker reports completion without sufficient verification | Independently supplement evidence; otherwise dispatch a fresh same-issue correction within budget |
| Restart with a committed issue but no result/status update | Resolve liveness first; reconstruct attributable evidence and verify, or use a bounded correction; never duplicate an uncertain live writer |
| Unrelated staged and unstaged changes | Preserve baseline contents and index state; use owned path-limited commits; independently inspect preservation |
| All unfinished issues externally blocked | Persist actionable evidence and stop without claiming completion or polling unchanged state |

Additional exercises covered cycles, missing recovery baselines, cross-ticket contamination by partial edits, correction counters across restarts/final integration, and changed specs invalidating completion evidence.

The review found two concrete defects, corrected before publication: contradictory missing-result recovery instructions, and an incorrect implication that a feature-local lock could provide checkout-wide exclusion. Recovery now explicitly permits a controller-reconstructed result only with independent evidence; exclusive checkout ownership is a prerequisite supplied by the runtime or a guaranteed single-controller environment. A feature-local lock is only a recovery lock, and ambiguous exclusivity stops writes.

## Live isolated-worker exercise

An independent controller ran the actual skill in a unique temporary Git repository. The fixture started with a Python greeting function and one passing unittest. Three real Markdown issues requested name normalization (01), an independent farewell function (02), and a personalized greeting that required 01 (03). Separate staged and unstaged sentinel files represented existing user work.

The controller dispatched three distinct native subagents with `fork_turns="none"`, in order 01 → 02 → 03. It waited for each to terminate and independently verified its result before dispatching the next. Only the workers edited application/test code after fixture setup. Each issue received a scoped commit, durable completed status, attempt/worker identity, SHA, and verification evidence. Tests progressed from 1 to 4, 6, then 8 passing cases.

Final commands passed:

```sh
python -m unittest discover -v
python -m compileall -q app.py test_app.py
git diff --check
```

The fixture has no configured type checker or linter; those checks were explicitly skipped, not reported as passing. `compileall` checks syntax, not types.

After every implementation worker terminated, a separate fresh read-only reviewer reran eight tests, checked normalization-before-greeting call order using in-memory spies, reviewed all three commit diffs, and compared sentinel hashes plus staged/unstaged patches with their original baselines. No acceptance, scope, or preservation findings remained. All application/test changes were committed; only issue metadata and the original sentinel changes remained dirty. The controller released its recovery lock. The parent also inspected the saved integration/review records and final Git status.

This live exercise covers sequential delegation, dependency ordering, per-issue verification, durable success records, dirty-file preservation, and final isolated review. Correction workers, forced crash recovery, external blockers, and mixed user/worker hunks in the same file were covered only by contract review, not by live fault injection. The isolated review and installation tests do not establish Claude Code execution behavior.

## Controller-epoch refactor (2026-09-27)

Issue #1 replaced the long-lived controller with restartable controller epochs. It also added durable `.orchestration/state.json`, fingerprint invalidation, write-ahead issue transitions, a fresh scheduler, a fresh read-only verifier (`references/verifier-contract.md`), and a filesystem mailbox with bounded agent returns. Everything else in this record describes the earlier version, unless this section says otherwise.

Static checks after the refactor: `quick_validate.py` accepted the skill, YAML frontmatter parsed, every relative Markdown link in the skill, README, and this record resolved, and `git diff --check` reported no whitespace errors.

An independent read-only agent traced the issue's ten stress scenarios against the refactored documents:

- A: 50 independent issues
- B: large issue bodies
- C: huge worker logs
- D: large diff
- E: pending issue edited mid-run
- F: new issue added
- G: controller killed after the worker commit
- H: controller killed after verification
- I: correction chain
- J: integration regression

The first trace found 14 defects. Four mattered most:

- `active_attempt` was never cleared, which blocked new selection.
- The worker-handle write bypassed the write-ahead protocol and desynchronized recorded fingerprints.
- Recovery could absorb an external issue edit as the controller's own.
- Integration had no durable phase for recovery.

All 14 were corrected: exact before/after hashes in `pending_transition`, per-verifier result directories, a persisted `verifier_retries` and `last_attempt`, durable integration phases, scheduler input fingerprints, attempt ids in payloads, canonical feature-relative issue paths, and hash-only write confirmation. A second independent review confirmed those fixes and found eight new defects in them, all corrected. Four were significant: an `incomplete` verification marked as accepted, integration retries sharing one directory, an integration `failed` result rerunning forever, and no way to rebuild an in-flight attempt when `state.json` is lost. The remaining four were first-run ordering, controller metadata misread as unattributable, an unbounded verifier retry during recovery, and an unbounded scheduler rerun. No third review was run after these fixes.

These are decision traces, not live executions. No stress scenario was run with real agents. Whether nested dispatch works for the driver re-entry mode depends on the host. Context bounds are contract targets (about 1500 tokens for worker and verifier results, about 1000 for scheduler envelopes), not measured values.

## Reproduce installation validation

Use a fresh temporary project for each agent. Run the local add/list commands above, inspect all installed files, parse `SKILL.md` frontmatter, and resolve its relative links. Avoid `--global` for this test. Delete only the temporary directories created for the test, after checking their resolved paths stay under the designated temporary parent.

To check the published GitHub source without installing:

```sh
npx --yes skills@latest add Anuise/issue-orchestrator --list
```

## GitHub publication checks

Published the implementation commit `813ad6a0dc92ffcc41ba889e45ece42688a887ab` to the existing `origin/main`; `git ls-remote` confirmed the same SHA. Then ran the discovery command above against the actual GitHub shorthand: exactly one skill, `issue-orchestrator`, was found.

Reinstalled from GitHub in both temporary agent projects:

```sh
npx --yes skills@latest add Anuise/issue-orchestrator --skill issue-orchestrator --agent claude-code --copy --yes
npx --yes skills@latest add Anuise/issue-orchestrator --skill issue-orchestrator --agent codex --copy --yes
```

Both succeeded. All five installed files matched source contents after normalizing Git's Windows CRLF conversion; raw byte comparison differed only in line endings. YAML parsing and every installed relative reference passed. The local-source installation comparison earlier in this record was byte-for-byte.

With the GitHub-source Codex installation, also ran:

```sh
npx --yes skills@latest update issue-orchestrator --project --yes
npx --yes skills@latest list --agent codex --json
```

The update succeeded (`Updated 1 skill(s)`), and listing identified `Anuise/issue-orchestrator` as a project-scoped GitHub source. Rechecked installed contents after the update. This tests the update operation on the current published version, not migration from an older version. Global installations were not touched.

## Cleanup limitation

The two test directories were verified as direct children of the system temporary directory, with no reparse points. Automatic approval policy rejected recursive deletion, including a narrower attempt naming one exact verified directory, with `blocked by policy`. The directories were retained rather than bypassing that restriction. Their names are `issue-orchestrator-install-750b9896ecec436aa2550fc4ba5f9916` and `issue-orchestrator-live-6512255f38fc4aa1977f16a5f5e10fde`; they contain only this validation's temporary installations, fixture, and evidence. Manual cleanup remains outstanding.

## Limits

Installation layout compatibility does not prove a host has isolated delegation enabled. No live Claude Code session was exercised. The runtime must provide fresh worker contexts, observe worker termination, and establish exclusive checkout ownership. The skill is not a daemon and cannot guarantee progress through host shutdown or exhausted usage limits. It deliberately has no concurrent feature scheduling or distributed locking service.
