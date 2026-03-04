#!/usr/bin/env bash

set -euo pipefail

usage() {
  cat <<'USAGE'
Usage:
  scripts/prompt-eval.sh --model <profile> --prompts <file> [--out <dir>] [--buddy-bin <path>]

Runs one-shot `buddy exec` prompt probes against a real configured model profile.
Each prompt line in <file> becomes one probe run with trace + response artifacts.

Options:
  --model      Model profile name (required)
  --prompts    Path to newline-delimited prompts file (required)
  --out        Output directory (default: artifacts/prompt-evals/<utc timestamp>)
  --buddy-bin  Buddy binary/command (default: buddy)
USAGE
}

MODEL=""
PROMPTS_FILE=""
OUT_DIR=""
BUDDY_BIN="buddy"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --model)
      MODEL="${2:-}"
      shift 2
      ;;
    --prompts)
      PROMPTS_FILE="${2:-}"
      shift 2
      ;;
    --out)
      OUT_DIR="${2:-}"
      shift 2
      ;;
    --buddy-bin)
      BUDDY_BIN="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown argument: $1" >&2
      usage
      exit 1
      ;;
  esac
done

if [[ -z "${MODEL}" || -z "${PROMPTS_FILE}" ]]; then
  echo "error: --model and --prompts are required" >&2
  usage
  exit 1
fi

if [[ ! -f "${PROMPTS_FILE}" ]]; then
  echo "error: prompts file not found: ${PROMPTS_FILE}" >&2
  exit 1
fi

if [[ -z "${OUT_DIR}" ]]; then
  OUT_DIR="artifacts/prompt-evals/$(date -u +%Y%m%dT%H%M%SZ)"
fi

mkdir -p "${OUT_DIR}"

RESULTS_TSV="${OUT_DIR}/results.tsv"
printf "id\tstatus\tprompt\ttrace\tresponse\tstderr\ttrace_summary\n" > "${RESULTS_TSV}"

probe_id=0
while IFS= read -r prompt || [[ -n "${prompt}" ]]; do
  trimmed="$(printf '%s' "${prompt}" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')"
  if [[ -z "${trimmed}" || "${trimmed}" == \#* ]]; then
    continue
  fi

  probe_id=$((probe_id + 1))
  trace_path="${OUT_DIR}/probe-${probe_id}.trace.jsonl"
  response_path="${OUT_DIR}/probe-${probe_id}.response.md"
  stderr_path="${OUT_DIR}/probe-${probe_id}.stderr.log"
  summary_path="${OUT_DIR}/probe-${probe_id}.trace-summary.txt"

  status="ok"
  if ! "${BUDDY_BIN}" --model "${MODEL}" --trace "${trace_path}" exec "${prompt}" > "${response_path}" 2> "${stderr_path}"; then
    status="error"
  fi

  if [[ -s "${trace_path}" ]]; then
    "${BUDDY_BIN}" trace summary "${trace_path}" > "${summary_path}" 2>> "${stderr_path}" || true
  else
    : > "${summary_path}"
  fi

  prompt_one_line="$(printf '%s' "${prompt}" | tr '\t\n' '  ')"
  printf "%s\t%s\t%s\t%s\t%s\t%s\t%s\n" \
    "${probe_id}" \
    "${status}" \
    "${prompt_one_line}" \
    "${trace_path}" \
    "${response_path}" \
    "${stderr_path}" \
    "${summary_path}" >> "${RESULTS_TSV}"
done < "${PROMPTS_FILE}"

echo "wrote prompt-eval artifacts:"
echo "  ${OUT_DIR}"
echo "results table:"
echo "  ${RESULTS_TSV}"
