# Prompt Evals (Real Models)

Use this workflow to evaluate prompt/tool wording changes against real providers
with reproducible artifacts and trace data.

## Purpose

1. Run a controlled set of prompts against a real configured profile.
2. Capture response output plus runtime traces per probe.
3. Compare model behavior after prompt/tool contract edits.

## Runner

Use the on-demand prompt-eval script:

```bash
make prompt-eval MODEL=<profile> PROMPTS=<file> [OUT_DIR=<dir>]
```

Equivalent direct command:

```bash
./scripts/prompt-eval.sh --model <profile> --prompts <file> [--out <dir>]
```

`PROMPTS` is a newline-delimited file:
- blank lines are ignored
- lines beginning with `#` are ignored

Each non-empty prompt line runs:
- `buddy --model <profile> --trace <probe-trace> exec "<prompt>"`

## Artifacts

Default output root:

`artifacts/prompt-evals/<utc-timestamp>/`

Per probe:

1. `probe-N.trace.jsonl`
2. `probe-N.trace-summary.txt`
3. `probe-N.response.md`
4. `probe-N.stderr.log`

Index:

`results.tsv` contains probe id, status, prompt text, and artifact paths.

## Iteration Loop

1. Run prompt evals with baseline prompt/tool wording.
2. Inspect `results.tsv` + trace summaries.
3. Drill into interesting runs with:

```bash
buddy trace replay <trace-file> --turn 1
buddy trace context-evolution <trace-file>
```

4. Adjust prompt/template/tool descriptions.
5. Re-run the same prompt set and compare outputs/traces.

## Notes

- This workflow is on-demand and intentionally not part of default `cargo test`.
- It uses your real auth/config setup for the selected profile.
- For tmux behavior changes, pair this with `make test-ui-regression`.
