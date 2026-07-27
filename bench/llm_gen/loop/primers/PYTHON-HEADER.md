# Python — generation context (give this to the model, then one task)

Target: **Python 3, single file, standard library only** (no third-party packages). The
file is run with `python3` exactly as you emit it — no human touches it.

Rules:
- Write the function with the EXACT name and signature the task requests; parameters and
  results are integers (`int`) unless the task says otherwise.
- The function must be correct for **every** input satisfying the task's stated
  preconditions — including negative values, zero, and very large magnitudes. Python's
  `int` is arbitrary-precision (no overflow) and `%`/`//` are floored (Euclidean-signed),
  which helps — but beware: **binary `float` silently drifts** (e.g. `int(4.35 * 100) == 434`,
  not `435`), so never route money/exact-decimal through `float`; and **integer division
  drops the remainder** (`n // k` loses `n % k` units — conserve the total explicitly).
- Emit ONLY the function definition, in ONE fenced code block, no prose outside it.
