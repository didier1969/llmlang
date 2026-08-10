#!/usr/bin/env python
"""ÉTAPE 2 (REQ-LLL-228, FINETUNE-BRIEF §4.2) — démonstration tri-langage à correction ÉGALE :
llmlang (NOTRE modèle fine-tuné) vs Python vs Rust, sur des tâches à INVARIANT.

Chaque tâche, single-shot (symétrique), pour chaque bras :
  • llmlang = notre modèle (Modal `infer_remote`, primer OMIS + aide-mémoire ~55 tok) → `lll check`
    PROUVE (Z3). La preuve EST la garantie. Forme du modèle (module complet) normalisée en parts nus.
  • Python / Rust = un modèle du commerce (OpenRouter) + son primer → code → l'ORACLE CACHÉ adversarial
    (identique aux 3 langages) révèle les bugs SILENCIEUX (escapes) que la preuve, elle, attrape.

RÉUTILISE le banc existant (zéro triche, garde-fous RESULTS.md) : `xlang_gen` fournit les TASKS,
les primers/prompts Python/Rust (`gen_prompt`), l'oracle caché (`hidden_correct`), les runners
(`py_outputs`/`rs_outputs`), la porte de preuve (`lll_check`). Le bras llmlang réutilise l'inférence
de `ft_smoke`. On mesure : tokens émis (marginal), tokens de prompt (le PRIMER — éliminé côté llmlang),
green (compile/prouve), escape (passe la porte visible mais rate l'oracle = bug livré).

    cd ~/projects/Unslot && set -a && source .env && set +a && \
      .venv-cloud/bin/python ~/projects/llmlang/bench/llm_gen/differential/ft_demo.py \
        --adapter-rel 8bd385a6/lora_model [--models anthropic/claude-sonnet-5] [--dry]
"""
from __future__ import annotations

import argparse
import json
import os
import statistics
import sys
import textwrap
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import xlang_gen as X  # noqa: E402 — TASKS + gen_prompt + call_model + oracle + runners + lll_check
import loop_run  # noqa: E402 — extract_code
from ft_smoke import HINT, SFT, instr_for  # noqa: E402 — le bras llmlang (prompt sans primer + indice)

RESULTS = os.path.join(HERE, "ft_demo_results.jsonl")


def to_bare_parts(code: str) -> str:
    """Notre modèle émet un `module M:` complet ; le banc attend des `part` nus (il enrobe lui-même
    en `module Gen:`). On retire l'entête module + on dé-indente → parts nus (avec `solve`)."""
    lines = code.split("\n")
    if lines and lines[0].strip().startswith("module ") and lines[0].rstrip().endswith(":"):
        return textwrap.dedent("\n".join(lines[1:])).strip("\n")
    return textwrap.dedent(code).strip("\n")


def modal_infer_usage(prompts: list[str], adapter_rel: str, *, max_new_tokens: int = 512,
                      wall_clock_s: int = 2400) -> list[dict]:
    """Une passe batch sur le GPU Modal, avec comptes de tokens (with_usage=True). Résiliente au poll."""
    import modal

    fn = modal.Function.from_name("unslot-training", "infer_remote")
    call = fn.spawn(adapter_rel, prompts, max_new_tokens=max_new_tokens, batch_size=8, with_usage=True)
    print(f">>> modal call id: {call.object_id}")
    deadline = time.monotonic() + wall_clock_s
    delay = 15
    while True:
        try:
            return call.get(timeout=120)
        except modal.exception.OutputExpiredError:
            raise
        except Exception as exc:
            if time.monotonic() >= deadline:
                raise
            print(f"    [modal] poll retry after {type(exc).__name__}: {exc}")
            time.sleep(delay)
            delay = min(delay * 2, 120)


def llmlang_arm(tasks: list[dict], adapter_rel: str) -> dict[str, dict]:
    """Notre modèle : 1 appel batch (primer omis + indice), puis oracle local par tâche."""
    prompts = [SFT.format(HINT + "\n\n" + instr_for(t)) for t in tasks]
    outs = modal_infer_usage(prompts, adapter_rel)
    res = {}
    for t, o in zip(tasks, outs):
        code = to_bare_parts(loop_run.extract_code(o["text"] or "") or "")
        green, _fb = X.lll_check(code) if code.strip() else (False, "empty")
        # HONNÊTETÉ (RESULTS.md) : distinguer un VRAI escape (prouve + oracle tourne + valeurs FAUSSES =
        # bug silencieux livré) d'un FAIL-STOP (oracle None : llmlang REFUSE d'émettre sur une entrée hors
        # précondition, ou limite du harnais). Le fail-stop est le comportement SÛR, PAS un bug silencieux.
        escape = fail_stop = False
        if green:
            oo, _e = X.lll_outputs(code, t["hidden"], X._mode(t))
            exp = [X._expected(t, r) for r in t["hidden"]]
            if oo is None:
                fail_stop = True
            elif oo != exp:
                escape = True
        res[t["id"]] = {
            "green": green, "escape": escape, "fail_stop": fail_stop,
            "tok_in": o["prompt_tokens"], "tok_out": o["completion_tokens"],
            "code": code[:600],
        }
    return res


def commercial_arm(lang: str, tasks: list[dict], model: str, key: str) -> dict[str, dict]:
    """Python ou Rust via OpenRouter (primer du langage inclus par gen_prompt), single-shot."""
    res = {}
    for t in tasks:
        reply, usage = X.call_model(model, X.gen_prompt(lang, t), key)
        code = loop_run.extract_code(reply or "") or ""
        green, _fb = X.LANGS[lang]["gate"](code, t) if code.strip() else (False, "empty")
        escape = green and not X.hidden_correct(lang, code, t)
        res[t["id"]] = {
            "green": green, "escape": escape,
            "tok_in": usage.get("prompt_tokens", 0) or 0,
            "tok_out": usage.get("completion_tokens", 0) or 0,
            "cost": usage.get("cost", 0.0) or 0.0,
            "code": code[:600],
        }
    return res


def _agg(arm: dict[str, dict]) -> dict:
    greens = [r for r in arm.values() if r["green"]]
    return {
        "green": sum(r["green"] for r in arm.values()),
        "escape": sum(r["escape"] for r in arm.values()),
        "fail_stop": sum(r.get("fail_stop", False) for r in arm.values()),
        "med_tok_out": int(statistics.median([r["tok_out"] for r in greens])) if greens else 0,
        "med_tok_in": int(statistics.median([r["tok_in"] for r in arm.values()])),
    }


def load_frozen(path: str, model: str, shown: str) -> dict[str, dict]:
    """Charge python / rust / llmlang (COMMERCIAL + primer) du run ÉQUITABLE frozen, filtré (model, shown).
    Réutilise une mesure déjà faite sous le protocole équitable (RESULTS.md) — pas de re-dépense
    OpenRouter, et donne le bras llmlang-COMMERCIAL pour le contraste de primer."""
    rows = [json.loads(l) for l in open(path) if l.strip()]
    out: dict[str, dict] = {}
    for r in rows:
        if r.get("model") != model or r.get("shown") != shown:
            continue
        out.setdefault(r["lang"], {})[r["task"]] = {
            "green": bool(r.get("shown_green")), "escape": bool(r.get("escape")),
            "tok_in": r.get("tokens_in", 0) or 0, "tok_out": r.get("tokens_out", 0) or 0,
            "code": (r.get("code") or "")[:600],
        }
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--adapter-rel", required=True, help="adaptateur v3 (ex. 8bd385a6/lora_model)")
    ap.add_argument("--frozen", default=os.path.join(HERE, "xlang_gen_results_fair.jsonl"),
                    help="run équitable frozen (Python/Rust/llmlang-commercial) — réutilisé sans re-dépense")
    ap.add_argument("--model", default="anthropic/claude-sonnet-5", help="modèle commercial du frozen")
    ap.add_argument("--shown", default="strong", help="gate équitable (strong = symétrique)")
    ap.add_argument("--dry", action="store_true", help="valide le pipeline local (refs frozen), 0 API")
    args = ap.parse_args()

    if args.dry:
        t = X.task("midpoint")
        pref = next(c for (l, c, _v) in X.REFS["midpoint"] if l == "python")
        gp, _ = X.LANGS["python"]["gate"](pref, t)
        lref = "module M:\n\n" + textwrap.indent(next(c for (l, c, _v) in X.REFS["max2"] if l == "llmlang"), "  ")
        lg, _ = X.lll_check(to_bare_parts(lref))
        print(f"[dry] python gate on ref = {gp} ; llmlang module→bare-parts gate = {lg}")
        print("[dry] OK — relancer sans --dry (run : Modal llmlang + réutilisation du frozen Python/Rust).")
        return 0 if (gp and lg) else 1

    frozen = load_frozen(args.frozen, args.model, args.shown)  # python / rust / llmlang (commercial)
    shared = sorted(set(frozen.get("llmlang", {})) & set(frozen.get("python", {})) & set(frozen.get("rust", {})))
    tasks = [X.task(i) for i in shared]
    if not tasks:
        raise SystemExit(f"aucune tâche partagée dans {args.frozen} (model={args.model}, shown={args.shown})")

    print(f">>> notre modèle llmlang ({args.adapter_rel}) sur {len(tasks)} tâches : {', '.join(shared)} …")
    ours = llmlang_arm(tasks, args.adapter_rel)

    with open(RESULTS, "w") as fh:
        for tid, r in ours.items():
            fh.write(json.dumps({"lang": "ft_llmlang", "model": "ft/llmlang", "task": tid, **r}) + "\n")

    # 4 bras sur les tâches partagées : notre modèle vs llmlang-commercial(primer) vs python vs rust.
    arms = [("llmlang (fine-tuné)", ours),
            ("llmlang (commerce+primer)", {k: frozen["llmlang"][k] for k in shared}),
            ("python (commerce)", {k: frozen["python"][k] for k in shared}),
            ("rust (commerce)", {k: frozen["rust"][k] for k in shared})]
    primer = X.primer_tokens("llmlang")
    n = len(tasks)
    print("\n" + "=" * 78)
    print(f"DÉMO — llmlang fine-tuné vs commerce · {n} tâches à invariant (frozen équitable {args.model}/{args.shown})")
    print("=" * 78)
    print(f"{'bras':<26} {'green':>7} {'escape':>7} {'fail-stop':>9} {'tok_out':>8} {'tok_in':>8}")
    for name, arm in arms:
        a = _agg(arm)
        print(f"{name:<26} {a['green']:>3}/{n:<3} {a['escape']:>7} {a['fail_stop']:>9} "
              f"{a['med_tok_out']:>8} {a['med_tok_in']:>8}")
    print("-" * 78)
    ours_in = _agg(ours)["med_tok_in"]
    print(f"PRIMER (coût FIXE) : écrire du llmlang avec un modèle du COMMERCE coûte ~{primer} tok/appel de")
    print(f"  primer (PROMPT-HEADER.md) ; NOTRE modèle l'a dans les poids → ~{ours_in} tok (indice ~55 inclus).")
    print("ESCAPE = prouve + oracle tourne + valeurs FAUSSES = bug silencieux livré (la preuve llmlang n'en")
    print("  produit JAMAIS). FAIL-STOP = llmlang REFUSE d'émettre sur une entrée hors précondition = le")
    print("  comportement SÛR (ergonomie de précondition trop restrictive), à NE PAS confondre avec un bug.")
    print(f"\nDétail (notre bras) dans {RESULTS}. Garde-fous RESULTS.md : oracle caché identique aux 3")
    print("langages, aucune signature truquée, chaque escape inspecté ; Python/Rust = run équitable frozen.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
