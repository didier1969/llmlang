#!/usr/bin/env python
"""Smoke OOD — le modèle llmlang fine-tuné généralise-t-il aux tâches du banc différentiel ?

ÉTAPE 1 de REQ-LLL-228 (gate avant toute dépense de comparaison). On feed chaque `spec` de tâche
(`xlang_gen.TASKS`, HORS-distribution pour le modèle : isqrt/midpoint/emod/…) au modèle fine-tuné
via Modal `infer_remote` (adaptateur `bfb7eb0f/lora_model`), **primer OMIS** (le langage est dans les
poids), puis l'oracle `lll check` + l'oracle caché tournent EN LOCAL. Révèle : green rate OOD, tokens
émis (marginal, estimé), le signal « primer éliminé » (prompt sans les ~12 Ko de PROMPT-HEADER.md),
et QUELLE forme le modèle émet (`part` nus vs module complet) — donnée nécessaire pour l'Étape 2.

RÉUTILISE `xlang_gen` (TASKS + signature + _example_str + _lll_module + lll_check + lll_outputs +
hidden_correct + LLL) : mêmes tâches, même oracle que la comparaison payante à venir. Zéro API sur les
bras Python/Rust (intacts). Lancer avec le python de .venv-cloud (il a `modal`) :

    cd ~/projects/Unslot && set -a && source .env && set +a && \
      .venv-cloud/bin/python ~/projects/llmlang/bench/llm_gen/differential/ft_smoke.py [--dry]
"""
from __future__ import annotations

import argparse
import os
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import xlang_gen as X  # noqa: E402  — TASKS + helpers + LLL (auto-câble loop_run → LLL_Z3)
import loop_run  # noqa: E402  — extract_code

ADAPTER_REL = "bfb7eb0f/lora_model"
SFT = "### Instruction:\n{}\n\n### Input:\n\n### Response:\n"


def instr_for(t: dict) -> str:
    """L'instruction llmlang de gen_prompt, MOINS le primer (le modèle connaît le langage)."""
    ex = "\n".join(X._example_str(t, r) for r in t["shown"])
    s = (t["spec"] + "\n\n# Required signature\n\n`" + X.signature("llmlang", t)
         + "`\n\n# Examples that must hold\n\n" + ex
         + "\n\nWrite it WITH a contract (`requires`/`ensures`) that CAPTURES the spec, so `lll check` "
           "proves it correct for every valid input. You may add helper `part`s.")
    if t.get("property"):
        s += f"\n\nYour `ensures` MUST establish this property: {t['property']}"
    s += "\n\nEmit ONLY the function/part definition(s) in ONE fenced code block, no prose outside it."
    return s


def check_forms(code: str) -> tuple[bool, str, str]:
    """Essaie les 2 formes. (a) part nus → enrobés par _lll_module ; (b) module complet verbatim.
    Retour (green, forme, feedback COMPLET de la forme qui rate le plus proche)."""
    ok_a, fb_a = X.lll_check(code)  # enrobe avec `module Gen:` (forme attendue par le banc)
    if ok_a:
        return True, "bare_parts", "green"
    with tempfile.TemporaryDirectory() as d:
        f = os.path.join(d, "m.lll")
        open(f, "w").write(code)
        r = subprocess.run([X.LLL, "check", "--no-cache", "--format=json", f],
                           capture_output=True, text=True, timeout=60)
        if r.returncode == 0:
            return True, "full_module", "green"
        fb_b = (r.stdout + r.stderr)
    return False, "neither", (f"[wrapped]{fb_a}\n[verbatim]{fb_b}")


def modal_infer(prompts: list[str], *, batch_size: int, max_new_tokens: int,
                adapter_rel: str = ADAPTER_REL, call_id: str | None = None,
                wall_clock_s: int = 2400) -> list[str]:
    import modal
    if call_id:
        call = modal.functions.FunctionCall.from_id(call_id)
        print(f">>> récupération de l'appel retenu {call_id} (0 inférence)")
    else:
        fn = modal.Function.from_name("unslot-training", "infer_remote")
        call = fn.spawn(adapter_rel, prompts, max_new_tokens=max_new_tokens, batch_size=batch_size)
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


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--dry", action="store_true",
                    help="valide le pipeline local (prompts + oracle lll) SANS appeler Modal (0 $)")
    ap.add_argument("--max-new-tokens", type=int, default=640)
    ap.add_argument("--batch-size", type=int, default=8)
    ap.add_argument("--call-id", default=None,
                    help="récupérer un appel Modal RETENU (0 inférence) au lieu d'en relancer un")
    ap.add_argument("--adapter-rel", default=ADAPTER_REL,
                    help="chemin de l'adaptateur RELATIF au volume (ex. <run>/lora_model)")
    args = ap.parse_args()

    tasks = X.TASKS
    prompts = [SFT.format(instr_for(t)) for t in tasks]
    primer_tok = X.primer_tokens("llmlang")
    ft_prompt_tok_est = [len(p) // 4 for p in prompts]

    print("=" * 68)
    print(f"SMOKE OOD — {len(tasks)} tâches du banc, modèle fine-tuné, primer OMIS")
    print("=" * 68)
    print(f"Tâches : {', '.join(t['id'] for t in tasks)}")
    print(f"Primer commercial (PROMPT-HEADER.md) ≈ {primer_tok} tok/appel — ÉLIMINÉ ici.")
    print(f"Prompt FT (sans primer) ≈ {min(ft_prompt_tok_est)}–{max(ft_prompt_tok_est)} tok (estim. chars/4).")
    print(f"\n--- exemple de prompt (tâche {tasks[0]['id']}) ---\n{prompts[0]}\n---")

    if args.dry:
        # Oracle local sur la réf FROZEN max2 (llmlang) = du llmlang GARANTI correct (verdict 'correct').
        ref = next(c for (lang, c, _v) in X.REFS["max2"] if lang == "llmlang")
        ok, form, fb = check_forms(ref)
        print(f"\n[dry] oracle local sur la réf frozen max2 (llmlang) → green={ok} forme={form} ({fb})")
        print("[dry] pipeline local OK — relancer sans --dry pour la mesure Modal (~0,30 $)."
              if ok else "[dry] ÉCHEC oracle local — corriger avant de dépenser.")
        return 0 if ok else 1

    print(f"\n>>> inférence Modal ({len(prompts)} prompts) …")
    completions = modal_infer(prompts, batch_size=args.batch_size,
                              max_new_tokens=args.max_new_tokens,
                              adapter_rel=args.adapter_rel, call_id=args.call_id)

    green = 0
    forms: dict[str, int] = {}
    fails: list[tuple[str, str, str]] = []  # (task_id, code, feedback) pour inspection
    print("\n" + "=" * 68)
    for t, prompt, comp in zip(tasks, prompts, completions):
        code = loop_run.extract_code(comp or "") or ""
        if not code.strip():
            ok, form, fb = False, "empty", "no code"
        else:
            ok, form, fb = check_forms(code)
        oracle = "n/a"
        if ok and form == "bare_parts":
            try:
                oracle = "correct" if X.hidden_correct("llmlang", code, t) else "WRONG(escape)"
            except Exception as exc:
                oracle = f"err:{type(exc).__name__}"
        forms[form] = forms.get(form, 0) + 1
        if ok:
            green += 1
        else:
            fails.append((t["id"], code, fb))
        tag = "✅" if ok else "❌"
        print(f"{tag} {t['id']:<14} form={form:<11} oracle={oracle:<14} out≈{len(code)//4:>3}tok")

    print("=" * 68)
    print(f"GREEN RATE OOD : {green}/{len(tasks)} = {100*green//max(1,len(tasks))}%")
    print(f"Formes émises  : {forms}")
    print(f"Primer éliminé : ~{primer_tok} tok/appel économisés (le langage est dans les poids).")

    if fails:
        print("\n" + "─" * 68 + "\nDÉTAIL DES ÉCHECS (inspecter avant de conclure — garde-fou honnêteté)")
        for tid, code, fb in fails:
            print("─" * 68 + f"\n### {tid} — code émis :\n{code}\n\n### erreur lll (extrait) :\n{fb[:600]}\n")

    print("\nLecture du gate : ≥~50% → généralise, on peut chiffrer la comparaison payante (Étape 2).")
    print("                  ≈0%   → in-distribution seulement → ajouter les familles du banc au corpus.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
