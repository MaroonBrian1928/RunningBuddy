# Advice quality evaluations

`advice_cases.json` contains synthetic, dated scenarios for short runs against
long-run plans, inferred weekdays, partial/empty weeks, missing sync coverage,
missing heart-rate benchmarks, weather comparisons, and historical follow-ups.
They contain no account data. The test helper builds the inputs with the same
schedule evaluation, weekly summary, and prompt builders used by the app.

## Offline regression checks

```sh
mise run api:test
mise run web:test
```

These check factual context, completion semantics, original snapshot separation,
response persistence, legacy compatibility, invalid evidence references, and UI
rendering. Passing them does **not** establish model advice quality.

## Generate answers for review

With the app's usual `LLM_PROVIDER`, `LLM_MODEL`, and provider key configured
(environment or `.env`):

```sh
mise exec -- cargo test live_advice_evaluation -- --ignored --nocapture
```

This explicitly sends the seven synthetic scenarios to the configured provider
and can incur API charges. It does not connect to the app database or write saved
advice. Each run writes a uniquely named JSON report in the system temporary
directory and prints its path. Reports include inputs, responses, provider/model,
and the criteria for each case. The runner validates the structured response
contract and known evidence IDs, but it does not grade factual claims or prose.
Provider/validation failures fail the run after the report is written.

## Score advice quality

Review each answer against its case criteria and score each dimension from 0–2:

| Dimension | 0 | 1 | 2 |
| --- | --- | --- | --- |
| Factual grounding | Invents or contradicts key facts | Mostly grounded, some unsupported interpretation | All material claims trace to supplied facts |
| Appropriate uncertainty | Treats missing/inferred data as certain | Mentions gaps but does not adjust advice | Gaps change the recommendation or trigger a focused question |
| Plan and date accuracy | Misstates completion, dates, or historical context | Minor ambiguity | Correct matching, period comparisons, and original/current distinction |
| Usefulness | Generic or unjustified prescription | Relevant but incomplete next step | Prioritized action with reason and clear adjustment condition |

Require every case-specific criterion to pass and no zero in any dimension;
aim for at least 7/8 per case. Treat fabricated benchmarks, a short jog declared
a completed long run, incomplete sync declared a missed workout, or current
facts substituted for an old review as a failed case regardless of total score.
Evidence IDs prove a cited source exists; reviewers must also check that it
actually supports the recommendation.

Keep before/after reports when changing prompts or models, use the same model
settings, and repeat runs to check variability. Record scores separately from
model output. There is no baseline quality score yet; the live runner must be
run and reviewed before claiming a measured quality improvement.
