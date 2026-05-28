# Signal Mapping Fix for Chisel-Generated Verilog

## Problem

BluesFL's signal tracing fails on NutShell's Chisel-generated Verilog because signal names change at module boundaries. Exact text matching (`get_text() == get_text()`) fails when comparing signals across Backend, WBU, EXU, ALU modules.

Current failure: Backend always block 274 returns `vars=None` because `wbu_io_in_bits_r_decode_cf_redirect_target` is not found in `output_nodes` via exact match.

## Approach: Incremental Suffix Matching

Add suffix-based fallback wherever exact signal name matching fails, reusing the existing `extract_signal_suffix` utility.

## Changes

### 1. `src/block/dfb.rs` — ModuleInput/ModuleOutput port mapping (lines 265-296)

**Current**: Exact match `block.outputs[].get_text() == port.get_text()`

**Change**: When exact match finds no `port_connection`:
1. Extract suffix of port name via `extract_signal_suffix`
2. Search `block.outputs`/`block.inputs` for nodes with `ends_with(suffix)`
3. Use matching port_connection

Applies to both ModuleInput (line 265) and ModuleOutput (line 282) branches.

### 2. `src/tracer/slice/dynamic_slice.rs` — get_driven_signals_in_block (lines 167-179)

**Current**: `local_sigs` filters `output_nodes` by exact match, returns None if empty

**Change**: When exact match yields empty `local_sigs`:
1. Extract suffix of `sig` via `extract_signal_suffix`
2. Search `output_nodes` for nodes with `ends_with(suffix)`
3. Use matches as `local_sigs`, then continue with coverage check

### 3. No changes to `extract_signal_suffix` (utils.rs)

Current 2-segment suffix extraction works for all Backend/WBU/EXU/ALU signals.

### 4. Logging

Add `warn!` logs when suffix fallback triggers, and when multiple matches found.

## Signal Examples

| Scope | Signal | Suffix |
|-------|--------|--------|
| Backend | `wbu_io_in_bits_r_decode_cf_redirect_target` | `redirect_target` |
| WBU | `io_in_bits_decode_cf_redirect_target` | `redirect_target` |
| WBU | `io_redirect_target` | `redirect_target` |
| EXU | `io_out_bits_decode_cf_redirect_target` | `redirect_target` |
| ALU | `io_redirect_target` | `redirect_target` |

All share suffix `redirect_target`, enabling cross-module matching.

## Risk

Suffix matching could produce false positives if unrelated signals share the same suffix. Mitigated by:
- Only triggering on exact match failure
- Logging when fallback activates
- Suffix must be >= 4 chars (existing guard)
