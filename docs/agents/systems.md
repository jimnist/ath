# Systems

This app talks to exactly one external system.

## Athletic.net

The single data source. Two surfaces:

- **Entries pages** - which athletes/teams are entered in each event
  (an Angular SPA; a JSON API likely backs it - see the handoff doc's open
  questions).
- **Athlete bio API** - `GetAthleteBioData`, the source of every mark.

Used for:

- Resolving the entrants for a meet/event
- Fetching each athlete's results and computing their best qualifying mark

Read more:

- `systems/athletic-net.md`
