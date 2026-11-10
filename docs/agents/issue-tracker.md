# Issue tracker

Issues live in **GitHub Issues** on [`jimnist/ath`](https://github.com/jimnist/ath/issues).

Use the `gh` CLI.
`gh api` (the REST API) works with a token that has only the `repo` scope; the higher-level `gh issue` and `gh pr` commands go through GraphQL and can fail without `read:org`, so the commands below use `gh api`.

## Reading issues

```bash
gh api --paginate 'repos/jimnist/ath/issues?state=open&per_page=100' --jq '.[] | select(.pull_request == null) | "#\(.number) \(.title) [\([.labels[].name] | join(", "))]"'
gh api --paginate 'repos/jimnist/ath/issues?labels=ready-for-agent&state=open&per_page=100' --jq '.[] | select(.pull_request == null) | "#\(.number) \(.title)"'
gh api repos/jimnist/ath/issues/42
gh api --paginate 'repos/jimnist/ath/issues/42/comments?per_page=100'
```

The issues endpoint also returns pull requests, so every listing keeps only entries without a `pull_request` field, and `--paginate` reads every page.

## Creating an issue

Write the body to a file (Markdown, one sentence per line) and pass it with `-F body=@<file>`, with one triage label and one type label (see `triage-labels.md`):

```bash
gh api repos/jimnist/ath/issues -f title='rankmap: ...' -F body=@issue.md -f 'labels[]=ready-for-agent' -f 'labels[]=feature'
```

The triage label is the one the creating skill says: `to-spec` and `to-tickets` publish `ready-for-agent`.
Issues opened from the issue templates (outside reporters) get `needs-triage` from the template.

## Updating an issue

```bash
gh api -X POST repos/jimnist/ath/issues/42/labels -f 'labels[]=ready-for-agent'
gh api -X DELETE repos/jimnist/ath/issues/42/labels/needs-triage
gh api -X POST repos/jimnist/ath/issues/42/comments -F body=@comment.md
gh api -X PATCH repos/jimnist/ath/issues/42 -f state=closed -f state_reason=completed
```

## Ticket dependencies

Use GitHub's **native issue dependencies**, not prose ([REST docs](https://docs.github.com/en/rest/issues/issue-dependencies)).
The API takes the blocking issue's `id` (the numeric database id, not its `#number`):

```bash
blocker=$(gh api repos/jimnist/ath/issues/12 --jq .id)
gh api -X POST repos/jimnist/ath/issues/15/dependencies/blocked_by -F issue_id="$blocker"   # #15 is blocked by #12
gh api --paginate 'repos/jimnist/ath/issues/15/dependencies/blocked_by?per_page=100' --jq '.[].number'   # what blocks #15
gh api --paginate 'repos/jimnist/ath/issues/12/dependencies/blocking?per_page=100' --jq '.[].number'     # what #12 blocks
gh api -X DELETE "repos/jimnist/ath/issues/15/dependencies/blocked_by/$blocker"
```

`to-tickets` publishes the tickets in dependency order and adds these links; a ticket is ready when every issue it is blocked by is closed.

## Pull requests

Pull requests are not a triage surface: they implement an issue and say so (`Closes #42`).
Open them against `main`; the pull request template lists the checks CI runs.

## Workflow

An issue is open until it is done, won't be done, or is a duplicate:

- It carries exactly one triage label at a time.
- Triage replaces `needs-triage` with `needs-info`, `ready-for-agent` or `ready-for-human`, or closes the issue with `wontfix` (`state_reason=not_planned`).
- A merged pull request with `Closes #nn` closes its issue.
- A duplicate is closed with a comment naming the original (`state_reason=duplicate`).
