# Working agreement: Nock Radio

This fork adds a worldwide internet radio tab to Nock. Two AI sessions work on it, and they cannot see each other. This repo is the only thing they share.

| Name | Where it runs | Does |
|---|---|---|
| **PM** | claude.ai, with Yuval | Spec, task briefs, review, merge |
| **Engineer** | Claude Code on Yuval's Windows PC, in this folder | Writes all code and tests, builds, runs, measures, opens PRs |

Yuval carries prompts from PM to Engineer and reports back.

## Who writes what (one writer per path)

| Path | Writer |
|---|---|
| `CLAUDE.md`, `docs/radio-spec.md`, `docs/radio-tasks.md` | PM only |
| `docs/results/*` | Engineer |
| Everything else (`src/`, `Cargo.toml`, tests, installer) | Engineer |

## Rules for the Engineer

1. Start every task with `git checkout main` and `git pull`, then make a new branch named `eng/<task-id>-<short-name>`.
2. Do the one task the prompt names, then stop. Do not start the next task.
3. Never edit the spec or the task list. If the spec is wrong or can't be built as written, stop and report it. Do not build around it.
4. Follow `CONTRIBUTING.md`: no polling loops, no new long-lived threads, no heavy dependencies, zero idle CPU, Win32/Direct2D/WinRT only. `WM_APP + n` numbers must stay unique (13 and up are free as of the fork).
5. Run `cargo build --release`, `cargo test` and `cargo clippy` before every push. Ship tests with code.
6. Write results to `docs/results/<task-id>.md`: what ran, what passed, what failed, what is untested, numbers measured.
7. Push the branch and open a PR into `main`. Do not merge it.
8. Run `git status` before each commit. Never commit logs, recordings, `%APPDATA%\Nock` files, tokens or anything personal.
9. If something fails twice for the same reason, stop and report.
10. Finish with a report that stands on its own: changes and why, results, untested items, branch and PR link.
