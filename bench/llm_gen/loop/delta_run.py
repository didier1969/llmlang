#!/usr/bin/env python3
"""delta_run.py — le harnais DELTA de CONTEXTE (REQ-LLL-192).

Mesure si donner à un LLM le contexte FOCALISÉ d'une définition cible (`lll
context`, le read-set du firewall de contrats DEC-LLL-017) l'aide à faire une
MODIFICATION VÉRIFIÉE d'un module llmlang existant avec moins de tokens que le
dump complet seul.

Tâche « modifier-un-module-sous-contexte » (distincte du banc spec→fonction de
loop_run.py). RÉUTILISE la machinerie task-agnostique de loop_run.py (call_model,
paired_ratio_stats, bootstrap_ci) et modélise la condition de contexte comme le
slot ARM : ARMS = ("LIVE", "DARK") — llmlang-only, donc PAS de cross langue×
contexte (qui casserait l'appariement) ; unit_key/pairing se réutilisent verbatim.

  DARK = primer + source COMPLÈTE du module + instruction de changement.
  LIVE = DARK + `lll context <file> <part> --format=json` (read-set minimal :
         source de la cible + les CONTRATS de ses dépendances directes, firewall).

Gate (le prédicat « changement présent », GENUINEMENT nouveau — rien dans loop_run
ne vérifie qu'une édition a atterri) : `lll check --no-cache` VERT *et* le(s)
marqueur(s) de changement présent(s) dans le module émis *et* le module tourne
encore (`lll run`). Une modif de module n'étant pas scalaire, on N'utilise PAS la
batterie held-out scalaire de loop_run.

Discipline de coût : `dryrun` n'utilise AUCUNE API (assemble les prompts, rapporte
le surcoût du contexte LIVE, et exerce le gate sur une modif de référence correcte
+ le module inchangé). `run` est GATED derrière BENCH_GO=1 + OPENROUTER_API_KEY,
exactement comme loop_run.cmd_run. Le run PAYANT attend un budget-go opérateur.

Suivi (PAS ici) : une fois l'IST .lll indexé dans Axon, ajouter un bras LIVE-AXON
injectant aussi Axon impact/why + l'intention SOLL (la thèse DEC-LLL-081). Constat
d'exploration : aujourd'hui Axon MCP N'indexe PAS les .lll (impact/why sur un
`part` = not-found ou faux-positifs vers le compilateur Rust) — donc LIVE utilise
`lll context`, qui calcule le vrai read-set EN DIRECT depuis le graphe d'appel.
"""
import argparse
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
# Réutilise la machinerie task-agnostique du banc spec→fonction (loop_run garde
# son `if __name__ == "__main__"`, donc l'import n'exécute que sa config).
from loop_run import (  # noqa: E402
    call_model,
    extract_code,
    clip,
    read_file,
    run_cmd,
    paired_ratio_stats,
    bootstrap_ci,
    unit_key,
    MODELS,
    SAMPLES,
    R_MAX,
    LLL,
    REPO,
    LLL_PRIMER,
)

ARMS = ("LIVE", "DARK")
TASKS_DIR = os.path.join(HERE, "delta_tasks")
RUNS_DIR = os.path.join(HERE, "runs")
RESULTS = os.path.join(HERE, "delta_results.jsonl")


# ------------------------------------------------------------------ tasks --

def load_tasks():
    with open(os.path.join(TASKS_DIR, "manifest.json")) as fh:
        return json.load(fh)["tasks"]


def base_path(task):
    return os.path.join(REPO, task["base"])


def base_src(task):
    return read_file(base_path(task))


def lll_context(task):
    """Le payload LIVE : `lll context <base> <target> --format=json`."""
    out = run_cmd([LLL, "context", base_path(task), task["target"], "--format=json"])
    return out.stdout if out.returncode == 0 else ""


# ---------------------------------------------------------------- prompts --

def gen_prompt(arm, task):
    core = (
        read_file(LLL_PRIMER)
        + "\n\n# Existing module\n\n```\n" + base_src(task) + "\n```\n\n"
        + "# Change to make\n\n" + task["instruction"] + "\n\n"
    )
    if arm == "LIVE":
        core += (
            "# Focused context — what to read to change the target safely "
            "(`lll context`: the target's source + the CONTRACTS of its direct "
            "dependencies, the verification firewall)\n\n```json\n"
            + lll_context(task) + "\n```\n\n"
        )
    core += (
        "Emit the COMPLETE modified llmlang module in ONE fenced code block. "
        "No prose outside the block."
    )
    return core


def repair_prompt(arm, task, code, feedback):
    return (
        "Your previous attempt FAILED verification or did not make the change.\n\n"
        "# Change to make\n\n" + task["instruction"] + "\n\n"
        "# Your previous attempt\n\n```\n" + code + "\n```\n\n"
        "# Failure\n\n```\n" + clip(feedback) + "\n```\n\n"
        "Emit the corrected, COMPLETE modified module in ONE fenced code block. "
        "No prose outside the block."
    )


# ------------------------------------------------------------------- gate --

def change_present(code, task):
    """La moitié GENUINEMENT nouvelle du gate : l'édition voulue a-t-elle atterri ?"""
    return all(marker in code for marker in task["change_markers"])


def gate_modify(code, tag, task):
    """VERT ssi : `lll check` exit 0 ET changement présent ET `lll run` marche."""
    os.makedirs(RUNS_DIR, exist_ok=True)
    path = os.path.join(RUNS_DIR, tag + ".lll")
    with open(path, "w") as fh:
        fh.write(code)
    chk = run_cmd([LLL, "check", "--no-cache", "--format=json", path])
    if chk.returncode != 0:
        return False, "lll check FAILED:\n" + clip(chk.stdout + chk.stderr)
    if not change_present(code, task):
        return False, "the required change did not land (markers absent): " + repr(task["change_markers"])
    run = run_cmd([LLL, "run", path])
    if run.returncode != 0:
        return False, "lll run FAILED (module no longer executes):\n" + clip(run.stdout + run.stderr)
    return True, "green"


# -------------------------------------------------------------- run + row --

def run_unit(task, model, sample, arm, key):
    base_tag = f"{task['id']}__{model.replace('/', '_')}__{arm}__{sample}"
    code, feedback, correct, rounds = "", "", False, 0
    tokens_in = tokens_out = 0
    cost = 0.0
    for rnd in range(1, R_MAX + 1):
        rounds = rnd
        prompt = gen_prompt(arm, task) if rnd == 1 else repair_prompt(arm, task, code, feedback)
        reply, usage = call_model(model, prompt, key)
        tokens_in += usage["prompt_tokens"]
        tokens_out += usage["completion_tokens"]
        cost += usage.get("cost", 0.0)
        code = extract_code(reply)
        with open(os.path.join(RUNS_DIR, base_tag + f"__r{rnd}.raw"), "w") as fh:
            fh.write(reply)
        correct, feedback = gate_modify(code, base_tag + f"__r{rnd}", task)
        if correct:
            break
    return {
        "pair": task["id"], "model": model, "sample": sample, "arm": arm,
        "correct": correct, "rounds": rounds,
        "tokens_in": tokens_in, "tokens_out": tokens_out,
        "tokens_total": tokens_in + tokens_out, "cost_usd": round(cost, 6),
        "r_max": R_MAX,
    }


def load_results():
    if not os.path.exists(RESULTS):
        return []
    with open(RESULTS) as fh:
        return [json.loads(line) for line in fh if line.strip()]


# --------------------------------------------------------------- commands --

def cmd_validate(_args):
    tasks = load_tasks()
    assert tasks, "no tasks in manifest"
    for t in tasks:
        for k in ("id", "base", "target", "instruction", "change_markers", "reference"):
            assert k in t, f"task {t.get('id')} missing `{k}`"
        assert os.path.exists(base_path(t)), f"base module missing: {t['base']}"
        assert os.path.exists(os.path.join(HERE, t["reference"])), f"reference missing: {t['reference']}"
        assert isinstance(t["change_markers"], list) and t["change_markers"], "change_markers must be non-empty"
    print(f"✔ {len(tasks)} task(s) valid: base modules + reference fixtures present, fields complete.")


def cmd_dryrun(_args):
    """Assemble les prompts LIVE/DARK, rapporte le surcoût du contexte, et exerce
    le gate sur la référence correcte + le module inchangé. AUCUNE API."""
    for task in load_tasks():
        print(f"\n=== task {task['id']}  (base {task['base']}, target `{task['target']}`) ===")
        dark = gen_prompt("DARK", task)
        live = gen_prompt("LIVE", task)
        ctx = lll_context(task)
        print(f"  DARK prompt : {len(dark):6d} chars")
        print(f"  LIVE prompt : {len(live):6d} chars  (+{len(live) - len(dark)} = le payload `lll context`, {len(ctx)} chars)")
        # Gate demo — SANS API : correct → VERT ; inchangé → ROUGE (l'édition n'a pas atterri).
        ref = read_file(os.path.join(HERE, task["reference"]))
        ok_ref, msg_ref = gate_modify(ref, f"dryrun_{task['id']}_reference", task)
        print(f"  gate(référence correcte) : {'VERT' if ok_ref else 'ROUGE'}  ({'lll check + changement présent + tourne' if ok_ref else msg_ref[:70]})")
        ok_base, msg_base = gate_modify(base_src(task), f"dryrun_{task['id']}_unchanged", task)
        verdict_base = "ROUGE (correctement)" if not ok_base else "VERT — INATTENDU"
        print(f"  gate(base inchangée)     : {verdict_base}  ({msg_base[:70] if not ok_base else ''})")
        assert ok_ref and not ok_base, "invariant dry-run : référence→vert, inchangée→rouge"
    print("\n✔ dry-run OK — prompts LIVE/DARK assemblés, surcoût contexte rapporté, gate changement-présent")
    print("  distingue correct vs inchangé. ZÉRO appel API. Pour le chiffre : BENCH_GO=1 delta_run.py run.")


def cmd_run(_args):
    if os.environ.get("BENCH_GO") != "1":
        raise SystemExit("GATED : BENCH_GO=1 requis pour dépenser des tokens (run payant). `dryrun` est gratuit.")
    key = os.environ.get("OPENROUTER_API_KEY")
    if not key:
        raise SystemExit("OPENROUTER_API_KEY requis pour un run payant.")
    tasks = load_tasks()
    done = {unit_key(r) for r in load_results() if "error" not in r}
    with open(RESULTS, "a") as fh:
        for task in tasks:
            for model in MODELS:
                for sample in range(SAMPLES):
                    for arm in ARMS:
                        if (task["id"], model, sample, arm) in done:
                            continue
                        try:
                            row = run_unit(task, model, sample, arm, key)
                        except SystemExit:
                            raise  # MAX_CALLS hit — stop hard
                        except Exception as exc:  # noqa: BLE001
                            row = {"pair": task["id"], "model": model, "sample": sample, "arm": arm, "error": str(exc)}
                        fh.write(json.dumps(row) + "\n")
                        fh.flush()
    print("run complete →", RESULTS)


def cmd_score(_args):
    rows = load_results()
    if not rows:
        raise SystemExit("aucun résultat — lancer d'abord `BENCH_GO=1 delta_run.py run`.")
    for arm in ARMS:
        n = sum(1 for r in rows if r.get("arm") == arm and "error" not in r)
        green = sum(1 for r in rows if r.get("arm") == arm and r.get("correct"))
        print(f"  {arm}: {green}/{n} vert")
    pair_medians, excluded, total = paired_ratio_stats(rows, "LIVE", "DARK")
    if not pair_medians:
        print(f"  aucune unité LIVE/DARK appariée & toutes-deux-correctes (exclues {excluded}/{total}) — non concluant.")
        return
    import statistics
    med = statistics.median(pair_medians)
    lo, hi = bootstrap_ci(pair_medians)
    print(f"  ratio tokens LIVE/DARK apparié : médiane {med:.3f}  IC95% [{lo:.3f}, {hi:.3f}]  (exclues {excluded}/{total})")
    print("  → LIVE consomme MOINS de tokens (delta positif)" if hi < 1.0 else "  → non concluant (l'IC inclut 1.0)")


def main():
    ap = argparse.ArgumentParser(description="Harnais delta de contexte (REQ-LLL-192) — modifier-un-module-sous-contexte, LIVE(`lll context`) vs DARK.")
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("validate", help="valider le manifest + les fixtures (gratuit)").set_defaults(fn=cmd_validate)
    sub.add_parser("dryrun", help="assembler prompts + exercer le gate, SANS API (gratuit)").set_defaults(fn=cmd_dryrun)
    sub.add_parser("run", help="run PAYANT apparié LIVE/DARK (BENCH_GO=1 requis)").set_defaults(fn=cmd_run)
    sub.add_parser("score", help="ratio apparié LIVE/DARK + IC bootstrap").set_defaults(fn=cmd_score)
    args = ap.parse_args()
    args.fn(args)


if __name__ == "__main__":
    main()
