# Triage labels

The five canonical triage roles, with their default names (no overrides).
They are **GitHub labels** on `jimnist/ath`, applied with `gh api` (see `issue-tracker.md`).

| Role | Label | Meaning |
|---|---|---|
| needs-triage | `needs-triage` | maintainer needs to evaluate |
| needs-info | `needs-info` | waiting on the reporter |
| ready-for-agent | `ready-for-agent` | fully specified, AFK-ready for an agent |
| ready-for-human | `ready-for-human` | needs human implementation |
| wontfix | `wontfix` | will not be actioned |

An issue carries exactly one triage label at a time:

- An issue an outside reporter opens (from an issue template) starts with `needs-triage`.
- An issue an agent creates gets the one label its skill says: `to-spec` and `to-tickets` publish `ready-for-agent`.
- Triage replaces the label; it never adds a second one.

## Type labels

Orthogonal to the triage labels: an issue carries one of each.

| Label | Meaning |
|---|---|
| `bug` | something works wrong |
| `feature` | something new, such as a State's Area file |
| `improvement` | something that works, made better |

GitHub label names are case-insensitive, and every new GitHub repo has a lowercase `bug`, so the names are lowercase.

## Creating the labels

On a fresh repo (`jimnist/ath` or a fork), create the ones that are missing (`wontfix` and `bug` exist by default).
Set `repo` to your own `owner/name` first; `gh repo view --json nameWithOwner --jq .nameWithOwner` prints it in a clone:

```bash
repo=$(gh repo view --json nameWithOwner --jq .nameWithOwner)
gh api "repos/$repo/labels" -f name=needs-triage -f color=fbca04 -f description='Maintainer needs to evaluate'
gh api "repos/$repo/labels" -f name=needs-info -f color=d4c5f9 -f description='Waiting on the reporter'
gh api "repos/$repo/labels" -f name=ready-for-agent -f color=0e8a16 -f description='Fully specified, ready for an agent'
gh api "repos/$repo/labels" -f name=ready-for-human -f color=1d76db -f description='Needs human implementation'
gh api "repos/$repo/labels" -f name=feature -f color=a2eeef -f description='Something new'
gh api "repos/$repo/labels" -f name=improvement -f color=c5def5 -f description='Something that works, made better'
```
