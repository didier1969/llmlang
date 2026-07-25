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
import time
import urllib.request
import urllib.error

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
# Réutilise la machinerie task-agnostique du banc spec→fonction (loop_run garde
# son `if __name__ == "__main__"`, donc l'import n'exécute que sa config). `loop_run`
# lui-même est importé comme module pour partager sa BORNE d'appels globale (`_calls`
# / `MAX_CALLS`) et son endpoint, tout en utilisant un `call_model` local à plus gros
# `max_tokens` (la tâche modify émet un module ENTIER ~2000 tokens, où le 2000 de
# loop_run TRONQUE → faux-censored).
import loop_run  # noqa: E402
from loop_run import (  # noqa: E402
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

# Budget de sortie : le module modifié complet (~2000 tokens) + marge. loop_run.call_model
# plafonne à 2000 (adapté à une fonction unique, PAS à un module) → on le remplace localement.
MAX_OUT_TOKENS = int(os.environ.get("DELTA_MAX_TOKENS", "6000"))


def call_model(model, prompt, key):
    """Complétion isolée, budget de sortie adapté au module ENTIER. Partage la BORNE
    d'appels globale de loop_run (`_calls`/`MAX_CALLS`) — garde de coût commune."""
    if loop_run._calls >= loop_run.MAX_CALLS:
        raise SystemExit(f"hard call cap reached ({loop_run.MAX_CALLS}) — stopping before spend")
    loop_run._calls += 1
    body = json.dumps({
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
        "temperature": 0.2,
        "max_tokens": MAX_OUT_TOKENS,
    }).encode()
    req = urllib.request.Request(loop_run.ENDPOINT, data=body, headers={
        "Authorization": f"Bearer {key}",
        "Content-Type": "application/json",
        "HTTP-Referer": "https://llmlang.local/bench",
        "X-Title": "llmlang-delta-bench",
    })
    for attempt in range(2):
        try:
            with urllib.request.urlopen(req, timeout=240) as r:
                data = json.load(r)
            return data["choices"][0]["message"]["content"], data.get("usage", {})
        except urllib.error.HTTPError as e:
            if e.code == 429 and attempt == 0:
                time.sleep(5)
                continue
            raise
    raise RuntimeError("unreachable")


def clean_code(reply):
    """`extract_code` + robustesse : strip un fence résiduel (```lang en tête / ``` en queue)
    qu'une extraction imparfaite aurait laissé (défense ; avec un max_tokens correct c'est rare)."""
    code = extract_code(reply)
    lines = code.splitlines()
    while lines and lines[0].lstrip().startswith("```"):
        lines = lines[1:]
    while lines and lines[-1].strip().startswith("```"):
        lines = lines[:-1]
    return "\n".join(lines)


# ── MODE SPLICE (DELTA_SPLICE=1) : le modèle n'émet QUE la/les `part` changée(s), pas tout le
# module — et les bras LIVE reçoivent le FOCUS SEUL (`lll context`) AU LIEU du dump complet. C'est
# le vrai test « focus économise des tokens » (le module vérifié permet un read-set serré qui
# SUFFIT). Le harnais re-splice la/les part émise(s) dans la base, puis `lll check`. Self-checké :
# splicer la part de la référence dans la base RE-produit la référence prouvée (les 5 tâches).
# (`SPLICE` est défini dans le bloc de config en tête.) ──


def _part_span(lines, name):
    start = None
    for i, l in enumerate(lines):
        s = l.strip()
        if s.startswith(f"part {name}(") or s.startswith(f"part {name} "):
            start = i
            break
    if start is None:
        return None
    end = start + 1
    for j in range(start + 1, len(lines)):
        l = lines[j]
        if l.strip() == "":
            end = j + 1
            continue
        if (len(l) - len(l.lstrip())) > 2:
            end = j + 1
        else:
            break
    while end > start + 1 and lines[end - 1].strip() == "":
        end -= 1
    return (start, end)


def extract_part(src, name):
    lines = src.split("\n")
    sp = _part_span(lines, name)
    return "\n".join(lines[sp[0]:sp[1]]) if sp else None


def split_emitted_parts(code):
    """Extrait chaque bloc `part X…:` d'un snippet émis (peut en contenir plusieurs)."""
    lines = code.split("\n")
    out, cur, ind = [], None, None
    for l in lines:
        s = l.strip()
        if s.startswith("part ") and (len(l) - len(l.lstrip())) <= 2:
            if cur is not None:
                out.append("\n".join(cur).rstrip())
            cur, ind = [l], len(l) - len(l.lstrip())
        elif cur is not None:
            cur.append(l)
    if cur is not None:
        out.append("\n".join(cur).rstrip())
    return out


def splice_parts(base_src, blocks):
    """Remplace dans `base_src` chaque `part` nommée par le bloc émis correspondant."""
    lines = base_src.split("\n")
    for blk in blocks:
        nm = None
        for bl in blk.split("\n"):
            s = bl.strip()
            if s.startswith("part "):
                nm = s[5:].split("(")[0].split()[0]
                break
        if nm is None:
            continue
        sp = _part_span(lines, nm)
        if sp is None:
            continue
        lines = lines[:sp[0]] + blk.split("\n") + lines[sp[1]:]
    return "\n".join(lines)

ARMS = ("DARK", "LIVE_CTX", "LIVE_AXON")
TASKS_DIR = os.path.join(HERE, "delta_tasks")
RUNS_DIR = os.path.join(HERE, "runs")
# MODE SPLICE (DELTA_SPLICE=1) : le modèle n'émet QUE les part(s) changée(s), et les bras LIVE
# reçoivent le FOCUS SEUL (`lll context`) au lieu du dump complet — le vrai test « focus économise
# des tokens ». Résultats dans un fichier SÉPARÉ (mêmes unit_keys que le mode full-module).
SPLICE = os.environ.get("DELTA_SPLICE") == "1"
RESULTS = os.path.join(HERE, "delta_results_splice.jsonl" if SPLICE else "delta_results.jsonl")


# ------------------------------------------------------------------ tasks --

def load_tasks():
    with open(os.path.join(TASKS_DIR, "manifest.json")) as fh:
        tasks = json.load(fh)["tasks"]
    only = os.environ.get("DELTA_ONLY")  # sous-ensemble d'ids, séparés par virgule
    if only:
        keep = set(only.split(","))
        tasks = [t for t in tasks if t["id"] in keep]
    return tasks


def base_path(task):
    return os.path.join(REPO, task["base"])


def base_src(task):
    return read_file(base_path(task))


def lll_context(task):
    """Le payload LIVE_CTX : `lll context <base> <target> --format=json` (source cible +
    contrats des CALLEES, le firewall)."""
    out = run_cmd([LLL, "context", base_path(task), task["target"], "--format=json"])
    return out.stdout if out.returncode == 0 else ""


def axon_block(task):
    """Le SUPPLÉMENT LIVE_AXON : le blast-radius `impact` d'Axon = les CALLERS/symboles
    impactés (ce que `lll context`, callee-only, n'a PAS). `axon_affects` est pré-capturé
    depuis `impact <target>` (project=LLL). VIDE quand Axon ne résout pas la cible
    (indexation .lll inégale) → LIVE_AXON dégrade alors vers LIVE_CTX."""
    affects = task.get("axon_affects", [])
    if not affects:
        return ""
    return (
        "# Axon impact analysis — changing `" + task["target"] + "` structurally AFFECTS these "
        "dependent symbols (CALLERS you must keep consistent; the `lll context` read-set above "
        "only shows the target's CALLEES): "
        + ", ".join(affects) + "\n\n"
    )


# ---------------------------------------------------------------- prompts --

def _ctx_block(task):
    return (
        "# Focused context — what to read to change the target safely "
        "(`lll context`: the target's source + the CONTRACTS of its direct "
        "dependencies, the verification firewall)\n\n```json\n"
        + lll_context(task) + "\n```\n\n"
    )


def gen_prompt(arm, task):
    primer = read_file(LLL_PRIMER)
    if SPLICE:
        # DARK reçoit le module COMPLET ; LIVE reçoit le FOCUS SEUL (le read-set serré) au lieu du
        # dump — c'est LÀ que le focus économise des tokens. Tous émettent SEULEMENT les parts changées.
        if arm == "DARK":
            ctx = "# Existing module\n\n```\n" + base_src(task) + "\n```\n\n"
        else:
            ctx = _ctx_block(task)
            if arm == "LIVE_AXON":
                ctx += axon_block(task)
        return (
            primer + "\n\n" + ctx + "# Change to make\n\n" + task["instruction"] + "\n\n"
            + "Emit ONLY the complete `part` definition(s) you change — the full "
            "`part NAME(...): … yield …` block(s), nothing else (NOT the whole module). "
            "One fenced code block, no prose outside it."
        )
    # ── mode module-complet (LIVE = DARK + contexte) ──
    core = (
        primer + "\n\n# Existing module\n\n```\n" + base_src(task) + "\n```\n\n"
        + "# Change to make\n\n" + task["instruction"] + "\n\n"
    )
    if arm in ("LIVE_CTX", "LIVE_AXON"):
        core += _ctx_block(task)
    if arm == "LIVE_AXON":
        core += axon_block(task)
    core += (
        "Emit the COMPLETE modified llmlang module in ONE fenced code block. "
        "No prose outside the block."
    )
    return core


def repair_prompt(arm, task, code, feedback):
    what = ("ONLY the corrected `part` block(s) you change (NOT the whole module)"
            if SPLICE else "the corrected, COMPLETE modified module")
    return (
        "Your previous attempt FAILED verification or did not make the change.\n\n"
        "# Change to make\n\n" + task["instruction"] + "\n\n"
        "# Your previous attempt\n\n```\n" + code + "\n```\n\n"
        "# Failure\n\n```\n" + clip(feedback) + "\n```\n\n"
        "Emit " + what + " in ONE fenced code block. No prose outside the block."
    )


# ------------------------------------------------------------------- gate --

def change_present(code, task):
    """La moitié GENUINEMENT nouvelle du gate : l'édition voulue a-t-elle atterri ?"""
    return all(marker in code for marker in task["change_markers"])


def gate_modify(code, tag, task):
    """VERT ssi : `lll check` exit 0 ET changement présent ET `lll run` marche.
    En mode SPLICE, `code` = les part(s) émise(s) → on les re-splice dans la base d'abord."""
    os.makedirs(RUNS_DIR, exist_ok=True)
    path = os.path.join(RUNS_DIR, tag + ".lll")
    if SPLICE:
        blocks = split_emitted_parts(code)
        if not blocks:
            return False, "aucun bloc `part` émis (mode splice)"
        code = splice_parts(base_src(task), blocks)  # module COMPLET après splice
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
        code = clean_code(reply)
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
    """Assemble les prompts des 3 bras, rapporte le surcoût de chaque étage de contexte,
    et exerce le gate sur la référence correcte + le module inchangé. AUCUNE API."""
    for task in load_tasks():
        print(f"\n=== task {task['id']}  (base {task['base']}, target `{task['target']}`, kind: {task.get('kind', '?')}) ===")
        dark = gen_prompt("DARK", task)
        ctx = gen_prompt("LIVE_CTX", task)
        axn = gen_prompt("LIVE_AXON", task)
        affects = task.get("axon_affects", [])
        print(f"  DARK      : {len(dark):6d} chars")
        print(f"  LIVE_CTX  : {len(ctx):6d} chars  (+{len(ctx) - len(dark)} = `lll context`, callees+contrats)")
        axon_note = f"impact→{', '.join(affects)}" if affects else "VIDE → dégrade vers LIVE_CTX (Axon ne résout pas la cible)"
        print(f"  LIVE_AXON : {len(axn):6d} chars  (+{len(axn) - len(ctx)} = blast-radius Axon : {axon_note})")
        # Gate demo — SANS API : correct → VERT ; inchangé → ROUGE (l'édition n'a pas atterri).
        ref = read_file(os.path.join(HERE, task["reference"]))
        ok_ref, msg_ref = gate_modify(ref, f"dryrun_{task['id']}_reference", task)
        print(f"  gate(référence correcte) : {'VERT' if ok_ref else 'ROUGE'}  ({'lll check + changement présent + tourne' if ok_ref else msg_ref[:70]})")
        ok_base, msg_base = gate_modify(base_src(task), f"dryrun_{task['id']}_unchanged", task)
        verdict_base = "ROUGE (correctement)" if not ok_base else "VERT — INATTENDU"
        print(f"  gate(base inchangée)     : {verdict_base}  ({msg_base[:70] if not ok_base else ''})")
        assert ok_ref and not ok_base, "invariant dry-run : référence→vert, inchangée→rouge"
    print("\n✔ dry-run OK — prompts DARK/LIVE_CTX/LIVE_AXON assemblés, surcoût de chaque étage rapporté,")
    print("  gate changement-présent distingue correct vs inchangé. ZÉRO appel API.")
    print("  Pour les 2 ratios (LIVE_CTX/DARK, LIVE_AXON/DARK) : BENCH_GO=1 delta_run.py run ; puis score.")


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
    import statistics
    for num in ("LIVE_CTX", "LIVE_AXON"):
        pair_medians, excluded, total = paired_ratio_stats(rows, num, "DARK")
        if not pair_medians:
            print(f"  {num}/DARK : aucune unité appariée & toutes-deux-correctes (exclues {excluded}/{total}) — non concluant.")
            continue
        med = statistics.median(pair_medians)
        lo, hi = bootstrap_ci(pair_medians)
        verdict = "MOINS de tokens (delta +)" if hi < 1.0 else "non concluant (IC inclut 1.0)"
        print(f"  {num}/DARK : ratio tokens médian {med:.3f}  IC95% [{lo:.3f}, {hi:.3f}]  (exclues {excluded}/{total}) → {verdict}")


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
