## Context

Direct receipt lookup measured 6.19 seconds warm and 19.64 seconds cold under
34.5 GB of swap use. Two production runs hit the ten-second ceiling on the same
valid receipt. Raising the bound is appropriate only after direct record lookup
and bounded startup transfer removed the two observed server-side amplifiers.

## Decision

Use a 30-second production request timeout. Continue to stop the entire memory
reconciliation pass on timeout or connection failure. Do not add an immediate
retry and do not weaken durable lookup-before-submit semantics.

## Uncomfortable fact

This accepts a longer outage-detection window. It does not make a slow ledger
fast; it gives measured valid operations enough time to finish while remaining
bounded.
