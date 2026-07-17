# Rust — generation context (give this to the model, then one task)

Target: **Rust 2021, single file, no external crates** (std only). The file is
compiled with `rustc --edition 2021` exactly as you emit it — no human touches it.

Rules:
- Write the function with the EXACT name and signature the task requests;
  integer parameters and results are `i64` unless the task says otherwise.
- The function must be correct for **every** input satisfying the task's stated
  preconditions — including negative values, zero, and values near
  `i64::MIN`/`i64::MAX`. Beware: Rust's `%` truncates toward zero, and release
  builds wrap on overflow silently.
- No `unsafe`, no I/O, no panics on valid inputs (unless the task explicitly
  allows a documented panic).
- Emit ONE fenced code block containing complete, compiling code. No prose
  outside the block.

*(This primer is deliberately minimal: Rust is in-distribution for every model
under test. Its token count is charged to the R arms' prompt-token accounting,
exactly as `PROMPT-HEADER.md` is charged to arm L — see PROTOCOL.md, primary
endpoint.)*
