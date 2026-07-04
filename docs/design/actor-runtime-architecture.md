# Architecture d'un runtime d'acteurs réel pour llmlang — design comparatif (BEAM / Tokio / Actix / Go / Pony)

> Concept d'implémentation durable. Alimente **REQ-LLL-036** (umbrella concurrence voie 2a) — paliers **W2 tiers 2+**, **W3** (supervision), **W4** (replay des entrelacements). Raffine **CPT-LLL-014** (modèle de concurrence à la frontière d'effet).
> Statut : recherche + architecture proposée. **Aucune ligne de code livrée par ce document.** Design à valider (GUI-PRO-021 design-twice appliqué aux décisions structurelles) avant tout incrément.
> Date : 2026-07-04.

---

## 0. Verdict en tête (réponse directe à la question opérateur)

**« Le runtime d'acteurs de llmlang peut-il égaler ou battre les bibliothèques de concurrence Rust/Go, et où dépasse-t-il plausiblement Elixir/BEAM ? »**

Réponse honnête, en trois strates :

1. **Égaler Rust/Go sur la mécanique brute : oui — parce qu'on construit DESSUS, pas contre.** Le tier-2 recommandé (voir §5) s'appuie sur `tokio` ou sur des threads OS + `std::sync::mpsc`. On **hérite** donc du scheduler work-stealing de Tokio (débit, équité, vol de tâches) ou de l'ordonnanceur du noyau OS — on ne le « bat » pas, la formulation « battre Tokio » est une erreur de catégorie : llmlang serait un **consommateur** de Tokio. La bonne cible n'est pas « plus rapide que Tokio » mais « ne dégrade pas ce que Tokio donne, tout en ajoutant les propriétés que Tokio n'offre pas » (isolation par acteur, replay).

2. **Dépasser BEAM : plausible sur exactement DEUX axes, pas plus.**
   - **(a) Une classe de bugs prouvée absente.** Chaque comportement d'acteur (`step`) est une part **totale, vérifiée Z3** (DEC-LLL-016/017) : elle termine, respecte ses pré/postconditions, et un invariant d'acteur (`state >= 0`) est prouvé **pour tout message**. Un `GenServer` Elixir non vérifié peut boucler, violer un invariant, lever silencieusement. Cet axe est **réel mais partiel** : il ne couvre que le cœur, pas la frontière d'effet (voir la nuance d'asymétrie ci-dessous).
   - **(b) Le replay déterministe de l'entrelacement est PLUS TRACTABLE pour llmlang que pour BEAM ou que pour le record-replay général.** C'est l'affirmation originale la plus forte du document (§7) : on **possède le scheduler** (on enregistre les décisions à la source, pas par instrumentation externe) ET les comportements sont **purs + totaux** (aucun non-déterminisme interne par acteur à capturer — seul l'ordre de livraison compte). L'« observer » de BEAM ne peut pas rejouer un entrelacement exact ; nous, si, par construction. **C'est la thèse porteuse du document.**

3. **Ne dépasse PAS BEAM, honnêtement :**
   - **Maturité / durcissement.** Des décennies de production (OTP, AXD301). Aucun document ne referme cet écart — c'est du temps et du track-record, pas du design. À nommer, pas à masquer (accord avec l'opérateur).
   - **Distribution multi-nœuds, largeur d'OTP** (gen_server, gen_statem, releases, hot upgrade éprouvé) : hors roadmap, esquissé seulement (§10).
   - **L'asymétrie critique de l'isolation.** BEAM isole *tout* (le cœur ET les effets sont dans le même modèle de processus). llmlang prouve le cœur mais l'**isolation ne protège que la frontière NON vérifiée** (§6) — or c'est précisément là que vivent les vrais crashes de production (I/O, FFI, `panic`). « Les bugs de logique prouvés absents » **ne touche pas** la classe de pannes qui domine en production. L'argument d'union (cœur ~0 défaut + frontière isolée-récupérable ≥ BEAM) **ne tient que si l'isolation de la frontière est étanche** — et `catch_unwind` n'est pas un filet universel (§6).

**En une phrase :** llmlang peut atteindre le niveau de Tokio/Go (en s'appuyant dessus) et peut *dépasser* BEAM sur la traçabilité rejouable et sur une classe de bugs prouvée absente, mais reste en retrait de BEAM sur la maturité et n'obtient la parité d'isolation que si la frontière d'effet reçoit un mécanisme d'isolation par acteur qui, en Rust, a des trous connus (`panic=abort`, unwinding à travers FFI).

---

## 1. État actuel (W2 slice 1) — le point de départ et le trou

Livré (commit `2ea91d0`, `examples/actor_runtime.lll` + `src/codegen.rs::emit_actor_runtime`) :

```rust
mod lll_actor_runtime {
    use std::sync::Mutex;
    static ACTORS: Mutex<Vec<i64>> = Mutex::new(Vec::new());
    pub fn spawn(initial: i64) -> i64 { /* push, retourne l'index comme Pid */ }
    pub fn send(pid: i64, msg: i64) {
        let mut a = ACTORS.lock().unwrap();
        a[idx] = super::lll_step(a[idx], msg);   // step INLINE, synchrone
    }
    pub fn state(pid: i64) -> i64 { /* lecture sous lock */ }
}
```

Restrictions **délibérées** de v1 (documentées dans le `.lll` et le code) :
- **Un seul comportement fixe par module** : une part littéralement nommée `step: (Int, Int) -> Int`, imposée et vérifiée au check-time (`src/types.rs`, `uses_actor_runtime` / `lll_actor_runtime`). Cause racine : passer un comportement **en valeur** à `spawn` exigerait de marshaller une **valeur fonctionnelle** à la frontière FFI — mécanisme inexistant (voir §9, famille REQ-LLL-052).
- **Messages `Int` uniquement** — même cause racine (pas de marshalling de valeurs ADT à la frontière ; gap tracé REQ-LLL-052).
- **`send` totalement synchrone** : appelle `step` en ligne, immédiatement, sous **un unique `Mutex` global** partagé par **tous** les acteurs. Aucun scheduling, aucune file, aucune concurrence, aucun parallélisme.

**Le trou de résilience n°1, à traiter en priorité :** il n'y a **aucune isolation de fautes**. Si le `step` compilé (ou n'importe quel bout du glue runtime) `panic!`, le panic **empoisonne le `Mutex` partagé** (`lock().unwrap()` → tous les `lock()` suivants renvoient `PoisonError` → `unwrap()` panique à son tour) et emporte **le processus entier, tous les acteurs d'un coup**. C'est aujourd'hui l'exact opposé de BEAM. Ce document fait de ce trou une préoccupation de première classe (§6).

**Trace/replay actuel** (`src/codegen.rs` ~1961) : `thread_local!` mono-thread, JSONL `{"eff":..,"v":..}` par effet performé, rejoué dans l'ordre. **Aucun horodatage logique, aucune identité acteur/message.** C'est ce mécanisme précis qu'on étend en §7 — pas un système parallèle.

**CORRECTION (2026-07-04, vérifiée empiriquement après relecture opérateur) :** cette section citait REQ-LLL-028 comme une aspérité round-trip encore ouverte sur un programme pur sans IO. C'était une lecture du corps SOLL périmé (le corps décrivait encore une proposition non implémentée), pas de l'état réel du code — `src/codegen.rs` ~1965-1987 a DÉJÀ le fix (`__lll_trace_init()` force la création eager du fichier de trace, l'ouverture REPLAY est tolérante à un fichier absent). Reproduit et confirmé : `lll run pure.lll --trace t && lll run pure.lll --replay t` → `[replay: OK]`, aucun panic. **REQ-LLL-028 N'EST PAS un prérequis de W4** — toutes les mentions ci-dessous (§7, annexe point 3) doivent se lire sans cette dépendance.

---

## 2. Systèmes de référence (recherche sourcée)

### 2.1 Erlang / BEAM — l'étalon de résilience

- **Isolation par heap privé.** Chaque processus BEAM a son propre heap et sa propre pile ; **aucune mémoire partagée**. Les messages sont **copiés** dans le heap du destinataire. Un processus ne détient donc jamais de référence dans la mémoire d'un autre → un crash ne corrompt rien à l'extérieur de lui-même. Le GC « n'a jamais à regarder au-delà d'un seul processus ». Sources : [erlang.org — message passing](https://www.erlang.org/blog/message-passing/), [erlang.org — Processes ref manual](https://erlang.org/documentation/doc-15.0/doc/system/ref_man_processes.html), [thèse Armstrong 2003](https://paperswelove.org/papers/making-reliable-distributed-systems-in-the-presenc-5ae3f98c/).
- **Ordonnanceur préemptif par comptage de réductions.** Une « réduction » ≈ un appel de fonction. Budget `CONTEXT_REDS` = **4000** (était **2000** avant OTP-20.0) ; à épuisement, le processus est préempté et remis en file. **Un thread d'ordonnancement par cœur**, **une run-queue par ordonnanceur**, équilibrage par **task-stealing + migration**. Aucun processus ne peut affamer les autres ni bloquer l'ordonnanceur. Source : [BEAM Book — scheduling](https://blog.stenmans.org/theBeamBook/) (mirroir : [raw asciidoc](https://raw.githubusercontent.com/happi/theBeamBook/master/chapters/scheduling.asciidoc)).
- **Arbres de supervision / OTP.** Stratégies **`one_for_one`** (redémarre le seul enfant tombé), **`one_for_all`** (redémarre tous), **`rest_for_one`** (redémarre l'enfant + ceux démarrés après lui), **`simple_one_for_one`** (enfants dynamiques identiques). Intensité max : au-delà de `MaxR` redémarrages en `MaxT` secondes le superviseur s'arrête lui-même (défauts `intensity=1`, `period=5`s) — anti-tempête de redémarrage. « Let it crash » : ne pas coder défensivement chaque erreur, laisser mourir et redémarrer depuis un état connu-bon. Sources : [Supervisor principles](https://www.erlang.org/doc/system/sup_princ.html), [supervisor module](https://www.erlang.org/doc/apps/stdlib/supervisor.html).
- **Distribution transparente.** Message-passing entre nœuds, liens et monitors « transparents quand on utilise des Pids » ; `epmd` pour la découverte, `net_kernel` pour les connexions. Source : [Distributed Erlang](https://www.erlang.org/doc/system/distributed.html).
- **D'où vient la résilience.** *Architecturalement* : isolation (heaps privés) + immutabilité + message-passing par copie → une faute ne peut corrompre un autre processus → on **redémarre** au lieu de **réparer** → les arbres de supervision transforment « crash » en « retour à un état connu-bon ». *Ce qui vient du durcissement* : les chiffres de disponibilité extrêmes reflètent aussi « beaucoup d'ingénierie de fiabilité dans le code C et le matériel » plus des décennies de rodage OTP. L'architecture rend la résilience *atteignable et bon marché* ; les nine-nines reflètent architecture **+** durcissement.
- **Le chiffre AXD301 « nine nines » (99.9999999 %) est mou** : extrapolation client (~14 nœuds, ~8 mois), pas une mesure de durée de vie rigoureuse ; « pas la seule chose qui fait tourner un AXD301 ». Sources : [DockYard — Reflections on the Erlang Thesis](https://dockyard.com/blog/2018/07/18/all-for-reliability-reflections-on-the-erlang-thesis), [Cronqvist — The nine nines](https://www.erlang-factory.com/upload/presentations/243/ErlangFactorySFBay2010-MatsCronqvist.pdf).

### 2.2 Tokio (Rust) — l'ordonnanceur qu'on consommerait

- **Work-stealing multi-thread.** Chaque worker a sa run-queue locale (buffer circulaire SP-MC de taille fixe **256**, `LOCAL_QUEUE_CAPACITY` [source](https://github.com/tokio-rs/tokio/blob/master/tokio/src/runtime/scheduler/multi_thread/queue.rs)) + une file globale d'injection. Un worker inactif **vole la moitié** de la queue d'un pair. **Slot LIFO** : une tâche qui devient runnable est placée dans un slot « next task » pour s'exécuter juste après l'émetteur du message (localité de cache). Source : [tokio.rs — 10x scheduler](https://tokio.rs/blog/2019-10-scheduler).
- **Ordonnancement coopératif.** Chaque tâche reçoit un budget de **128** opérations par tick ; passé le budget, toutes les ressources Tokio renvoient `Pending` jusqu'à ce que la tâche rende la main. Une tâche CPU-bound sans `.await` **affame** le runtime → `spawn_blocking`. Source : [tokio.rs — preemption](https://tokio.rs/blog/2020-04-preemption).
- **Isolation par tâche.** Un `panic!` dans une tâche `spawn`ée est **capturé par l'exécuteur — il ne tue pas le runtime** ; on l'observe via `JoinHandle` → `Err(JoinError)`, `is_panic()`/`into_panic()`. Nuance : le panic n'est **pas** re-propagé automatiquement — il faut inspecter le `JoinError` (sinon il est silencieusement isolé). Source : [docs.rs JoinError](https://docs.rs/tokio/latest/tokio/task/struct.JoinError.html).
- **Concurrence structurée.** `JoinSet` (drop = abort de toutes ses tâches), `tokio::select!` (première branche gagne, les autres sont *droppées* → sûreté d'annulation), annulation par drop. Canaux : `mpsc` borné (`send().await` bloque quand plein = **backpressure**), non-borné (pas de backpressure), `oneshot`. Sources : [JoinSet](https://docs.rs/tokio/latest/tokio/task/struct.JoinSet.html), [select!](https://docs.rs/tokio/latest/tokio/macro.select.html), [mpsc](https://docs.rs/tokio/latest/tokio/sync/mpsc/index.html).

### 2.3 Actix (Rust) — l'acteur sur Tokio, et ses limites

- **Modèle.** Tout type Rust peut être un acteur (`Actor`), mailbox par acteur, référencé par `Addr`/`Recipient`, un `Handler<M>` par type de message. Un **Arbiter = une boucle d'événements mono-thread** (runtime Tokio current-thread) ; les acteurs sont liés à leur Arbiter. Sources : [actor](https://actix.rs/docs/actix/actor/), [arbiter](https://actix.rs/docs/actix/arbiter/), [actix-rt](https://docs.rs/actix-rt/latest/actix_rt/).
- **Supervision faible vs BEAM.** `Supervised` + `Supervisor` : sur échec, **réutilise la même instance** avec un `Context` neuf (`restarting()`), pas de reconstruction ni d'arbre OTP (pas de stratégies one-for-all/rest-for-one, pas de child specs). Déclenché par arrêt du contexte, **pas** par capture de panic. Message en cours perdu → `Err(Cancelled)`. Source : [Supervisor](https://docs.rs/actix/latest/actix/struct.Supervisor.html).
- **Granularité d'isolation = l'Arbiter (thread), pas l'acteur.** Un `panic!` dans un handler **fait tomber tout l'Arbiter — donc tous les acteurs qui le partagent**. Par défaut les autres Arbiters survivent (sauf l'Arbiter système). C'est **plus faible que BEAM** (où chaque processus est isolé, heap séparé). Source : [arbiter](https://actix.rs/docs/actix/arbiter/), [actix#110](https://github.com/actix/actix/issues/110).

### 2.4 Go — goroutines + canaux (CSP), et son talon d'Achille

- **Scheduler G-M-P.** G (goroutine), M (thread OS), P (processeur logique = `GOMAXPROCS`), run-queue locale par P, **work-stealing** (vole la moitié). **Préemption asynchrone depuis Go 1.14** (signal `SIGURG`) — avant, coopérative aux points d'appel seulement. Pile initiale ~2 KB, croissante. Sources : [go1.14](https://go.dev/doc/go1.14), [scheduler design doc](https://go.dev/s/go11sched).
- **Propagation de panic — CRITIQUE.** Un `panic` non-`recover` remonte la pile de **la goroutine** puis « **le programme crashe** ». `recover` n'est utile **que** dans un `defer` de **la même goroutine** — il **ne peut pas** rattraper le panic d'une autre. Donc un panic non rattrapé dans **n'importe quelle** goroutine **termine le processus entier** — **aucune isolation par goroutine**, l'opposé de BEAM. Sources : [go.dev/blog defer-panic-recover](https://go.dev/blog/defer-panic-and-recover), [spec — Handling panics](https://go.dev/ref/spec#Handling_panics).
- **Ce que Go n'offre PAS vs BEAM.** Pas d'arbres de supervision, pas de `link`/`monitor`, pas de « let it crash » intégré. Mémoire **partagée** : les canaux sont une *discipline recommandée* (« share memory by communicating ») mais **advisory** — les data races sont possibles, détectées seulement au runtime par le race detector (`-race`), jamais prouvées absentes. Sources : [race detector](https://go.dev/doc/articles/race_detector), [blog race-detector](https://go.dev/blog/race-detector).

### 2.5 Pony — l'acteur data-race-free PAR LE TYPE (le plus proche de l'éthos llmlang)

- **Acteurs à heap privé, GC concurrent (ORCA), sans locks.** Comportements asynchrones ; « Pony n'a aucun lock … le système de types garantit **à la compilation** que votre programme concurrent ne peut jamais avoir de data race ». Sources : [actors](https://tutorial.ponylang.io/types/actors.html), [GC](https://tutorial.ponylang.io/appendices/garbage-collection.html).
- **Reference capabilities = liberté de data-race STATIQUE.** Six capacités (`iso`, `trn`, `ref`, `val`, `box`, `tag`) = *deny capabilities* exprimant ce que les *autres* alias ne peuvent pas faire. « Nous fournissons un système de types qui **garantit statiquement l'absence de data race** pour un langage à modèle d'acteurs. » Si un programme Pony **compile**, les data races sont **impossibles** — contrairement au détecteur runtime de Go. Sources : Clebsch et al., *Deny Capabilities for Safe, Fast Actors* (AGERE! 2015) [ACM](https://dl.acm.org/doi/10.1145/2824815.2824816) / [PDF](https://www.ponylang.io/media/papers/fast-cheap.pdf), [tutorial reference-capabilities](https://tutorial.ponylang.io/reference-capabilities/reference-capabilities.html).
- **Messagerie causale, scheduler work-stealing** (threads = nb de cœurs). **Modèle d'échec** : pas d'exceptions typées ; `error` + fonctions **partielles** marquées `?` — un chemin d'erreur non traité **ne compile pas**. Source : [errors](https://tutorial.ponylang.io/expressions/errors.html).

**Leçon Pony pour llmlang :** la parenté philosophique est directe — *prouver l'absence d'une classe de bugs à la compilation plutôt que la détecter au runtime*. llmlang le fait déjà pour la logique (Z3) ; l'analogue « pas de mémoire mutable partagée entre acteurs » doit être garanti **par construction du runtime** (état possédé par acteur, jamais un `Mutex` global — §5/§6), faute d'un système de capabilities dans le langage.

---

## 3. Contraintes llmlang (le cadre non négociable)

Tout design ci-dessous respecte, sans exception :

1. **La boucle de dispatch non-terminante ne peut PAS être une part vérifiée** (DEC-LLL-016 : toute part prouvée termine). Le *moteur* (threads/canaux, boucle infinie) est **du Rust runtime écrit à la main**, hors cœur. (Note opérateur REQ-LLL-036 : la *logique de décision pure* du scheduler — ordre de traitement, repli file→état, reconstruction d'entrelacement — **est** un bon candidat auto-hébergé et prouvé ; le moteur bas niveau, non.)
2. **La concurrence vit à la frontière d'effet** (CPT-LLL-014, DEC-LLL-017) : Z3 **havoc** le résultat de `spawn`/`send`/`state` — le cœur prouvé est sound quel que soit l'ordonnancement. Aucune modification de `vc.rs` requise par ce qui suit (à re-confirmer par recherche avant tout code, comme le tripwire de slice 1).
3. **Texte = source de vérité ; hash/caches = dérivés** (DEC-LLL-020/034). Pertinent pour le hot-reload (§11).
4. **Pas de mémoire mutable partagée entre acteurs, sauf par message** (invariant à *imposer par construction du runtime* — le `Mutex<Vec<i64>>` global de slice 1 le **viole** et doit disparaître).
5. **Zéro warning, zéro artefact prototype** (GUI-LLL-001, GUI-PRO-003) : le code généré doit compiler proprement.

---

## 4. Séquençage (tracer-bullet, GUI-PRO-023)

On ne propose **pas** une refonte monolithique. Ordre des tranches verticales :

| Tranche | Contenu | Débloque |
|---|---|---|
| **W2-t2** | Parallélisme réel : état possédé par acteur, mailbox par acteur, un vrai boucle de dispatch runtime (§5) | tout le reste |
| **W2-t2b** | Isolation de fautes à la frontière : `catch_unwind` par step + état local (§6) | W3 |
| **W3** | Supervision « let it crash » : politique de redémarrage runtime (§8) | — |
| **W4** | Replay déterministe des entrelacements : séquence logique globale au point de livraison (§7) | l'atout vs BEAM |
| **(gated)** | Comportements génériques + messages ADT riches : **bloqué par REQ-LLL-052** (§9) | lève les restrictions v1 |
| **(hors roadmap)** | Distribution (§10), hot-reload (§11) — analysés, non planifiés | — |

Chaque tranche garde chaque comportement **total + vérifié Z3** et reste rejouable à l'identique — DoD de l'umbrella.

---

## 5. Tier-2 : parallélisme réel — **design-twice (GUI-PRO-021)**

Objectif : supprimer le `Mutex<Vec<i64>>` global et le `send` synchrone. Chaque acteur possède son **propre état** et sa **propre mailbox** ; un moteur runtime livre les messages et applique `step` **hors du cœur vérifié**.

Trois candidats réels, comparés sans homme de paille.

### Candidat A — Threads OS + `std::sync::mpsc` par acteur (« un thread par acteur »)

- **Forme.** `spawn` crée un `std::thread` qui possède `state: i64` (jamais partagé) et boucle sur `rx.recv()` ; chaque message applique `state = lll_step(state, msg)`. `send(pid, msg)` = `tx.send(msg)` sur le canal de l'acteur. `state(pid)` = un aller-retour requête/réponse via un `oneshot` (ou un `Mutex<i64>` **par acteur**, pas global — lecture seule concurrente acceptable).
- **Fit modèle llmlang.** Excellent : la boucle non-terminante est un thread Rust pur (contrainte 1 respectée), l'état est **possédé** par le thread (contrainte 4 respectée par construction — pas de partage). `spawn`/`send`/`state` restent des effets extern havoc'd.
- **Isolation.** La plus forte des trois **naturellement** : chaque acteur = un thread OS avec sa propre pile ; un `catch_unwind` autour du `lll_step` (§6) confine le panic au thread, l'état de l'acteur peut être réinitialisé (« let it crash »), **les autres threads ne voient rien**. Le plus proche du heap-privé de BEAM.
- **Ordonnancement.** Délégué au **noyau OS** — équité correcte, préemptif, mais **un thread OS par acteur ne passe pas à l'échelle** (BEAM/Go/Tokio font des millions d'acteurs légers ; les threads OS plafonnent à ~10⁴–10⁵ et coûtent ~Mo de pile chacun). **C'est le défaut rédhibitoire pour la cible « classe BEAM ».**
- **Débit.** Bon à faible nombre d'acteurs, s'effondre à grande échelle (context-switch noyau, pression mémoire).
- **Verdict.** Idéal comme **jalon de correction** (le plus simple à rendre correct et isolé), mais ne peut pas être la cible finale. À considérer comme tranche W2-t2 *intermédiaire* si l'on veut prouver l'isolation avant l'échelle.

### Candidat B — Tâches async `tokio` + canaux `tokio::mpsc` (recommandé)

- **Forme.** Runtime Tokio multi-thread. `spawn` crée une **tâche** (`tokio::spawn`) possédant `state` et bouclant sur `rx.recv().await` ; mailbox = `tokio::mpsc` **borné** (backpressure gratuite). `send` = `tx.try_send`/`send().await`. `state` = `oneshot`. Le Pid = clé dans une table `Pid → Sender` (la table est la seule structure partagée, en lecture, derrière un `RwLock` ou une `DashMap` — **jamais** l'état des acteurs).
- **Fit modèle llmlang.** Très bon. La boucle non-terminante est une tâche async (Rust runtime, contrainte 1). État possédé par la tâche (contrainte 4). Effets havoc'd inchangés.
- **Isolation.** Un `panic!` dans une tâche est **capturé par Tokio** (§2.2) → `JoinError`, le runtime survit. **Mais** : la granularité est la *tâche*, pas un heap séparé ; un `panic=abort` (profil release possible) **contourne** la capture. On ajoute donc `catch_unwind` par step (§6) pour la politique de redémarrage. Isolation **≥ Actix** (qui isole à l'Arbiter/thread, pas à l'acteur — ici chaque acteur est sa propre tâche).
- **Ordonnancement / débit — le point clé.** On **hérite** du scheduler work-stealing de Tokio : run-queues locales de 256, vol de la moitié, slot LIFO qui fait tourner le destinataire juste après l'émetteur (localité de cache idéale pour le message-passing), budget coop de 128 (équité). **C'est exactement le profil qu'on veut, et on ne l'écrit pas** — on l'obtient. Millions de tâches légères, pas de plafond thread-OS. **Piège** : `lll_step` est **CPU-bound et sans `.await`** ; une rafale de messages pourrait affamer le worker. Mitigation : `lll_step` est *borné et court par construction* (part totale, mesure de terminaison prouvée) — le risque de famine est structurellement limité, et `tokio::task::coop::consume_budget()` peut être inséré dans la boucle de mailbox entre deux messages.
- **Verdict — RECOMMANDÉ.** Meilleur rapport (fit × isolation × échelle × débit hérité). C'est aussi le chemin qui rend vraie l'affirmation §0.1 (« on égale Rust/Go en construisant dessus »). Le seul coût : dépendance `tokio` dans le binaire généré (acceptable, déjà l'écosystème async standard de Rust).

### Candidat C — Scheduler green-thread coopératif custom (réductions à la BEAM)

- **Forme.** Écrire notre propre ordonnanceur : N threads workers = nb de cœurs, chacun une run-queue d'acteurs prêts, work-stealing maison, préemption par **comptage de réductions** (comme BEAM : après K applications de `step`, remettre l'acteur en file). État par acteur dans une structure `Actor { state, mailbox: VecDeque }`.
- **Fit modèle llmlang.** Bon en principe, et **conceptuellement le plus proche de BEAM**. La *logique de décision* (ordre, budget de réductions, migration) est même auto-hébergeable et prouvable (note opérateur REQ-LLL-036) — le moteur reste Rust.
- **Isolation.** À écrire entièrement à la main (`catch_unwind` par step + réinit d'acteur). Rien de gratuit.
- **Ordonnancement / débit.** *Potentiellement* le meilleur (contrôle total : on peut faire du replay-friendly by design, cf §7), mais **on réécrit ce que Tokio a déjà rendu 10× plus rapide après des années de tuning** ([tokio 10x](https://tokio.rs/blog/2019-10-scheduler)). Coût d'ingénierie et de durcissement énorme, risque de bugs de concurrence dans **notre** code (le comble pour un langage vérifié : le scheduler non vérifié devient le maillon faible).
- **Verdict.** **Rejeté pour le tier-2 initial** — sur-ingénierie prématurée, viole GUI-LLL-001 (zéro spike/prototype) en pratique car un scheduler maison mûr est un projet en soi. **Réservé** comme évolution *si et seulement si* une exigence de replay (§7) ou d'auto-hébergement du scheduler l'impose et que Tokio se révèle insuffisant. À ce moment-là, la *logique de décision pure* serait écrite en llmlang (prouvée), le moteur en Rust.

### Recommandation tier-2

**B (Tokio), avec A (threads OS) comme jalon d'isolation optionnel de plus petit risque si l'on veut prouver « zéro partage + isolation » avant de brancher l'échelle.** C exclu du chemin initial. Raison décisive : B est le seul qui **égale Rust/Go en héritant** de leur ordonnanceur au lieu de le concurrencer, tout en gardant chaque acteur isolable finement (une tâche par acteur, `catch_unwind` par step). La table `Pid → Sender` est la **seule** structure partagée, et uniquement en lecture — l'invariant « pas d'état mutable partagé entre acteurs » (contrainte 4) est tenu par construction.

**Dépendance de généricité :** aucun des trois candidats ne lève, à lui seul, la restriction « un seul `step`, messages `Int` ». Cette restriction est orthogonale au choix de moteur — elle dépend de REQ-LLL-052 (§9). B peut être livré **avec** la restriction v1 (un `step` fixe, `msg: i64` dans les canaux) : on gagne parallélisme + isolation **sans** attendre le marshalling ADT/fonctionnel. C'est le bon découpage tracer-bullet.

---

## 6. Isolation de fautes à la frontière d'effet (le trou présent)

**Problème concret :** aujourd'hui un panic empoisonne le `Mutex` global → tout meurt. Objectif : donner à la frontière **NON vérifiée** l'isolation universelle que BEAM donne à tout.

**Le fix est DEUX changements, pas un :**

1. **État possédé par acteur (supprimer le `Mutex` global).** Chaque acteur possède son `state` (dans son thread/tâche — candidat A ou B de §5). Un panic ne peut plus empoisonner un verrou partagé puisqu'il n'y a plus de verrou partagé. **C'est le changement structurel qui vaut le plus** : il transforme « une panne = mort globale » en « une panne = un acteur affecté ».
2. **`catch_unwind` par application de `step`.** Envelopper `lll_step(state, msg)` dans `std::panic::catch_unwind(AssertUnwindSafe(...))`. Sur `Err`, appliquer la politique de redémarrage (§8 : réinitialiser l'état à sa valeur initiale/dernier état connu-bon, journaliser, éventuellement notifier un superviseur) au lieu de propager.

**Caveats honnêtes (à ne PAS masquer) — pourquoi ce n'est pas le filet universel de BEAM :**
- `catch_unwind` **ne rattrape pas** les builds `panic = "abort"` (profil release courant pour la performance). Si llmlang compile en `panic=abort`, le mécanisme est inerte. → **Décision requise** : forcer `panic = "unwind"` pour les modules à runtime d'acteurs (coût perf marginal), ou accepter que l'isolation soit `unwind`-only.
- **Unwinding à travers une frontière FFI est un comportement indéfini (UB).** Or les vrais effets de production (`extern` I/O, FFI Rust) sont *exactement* la frontière non vérifiée. Un panic qui traverse un `extern "C"` n'est pas rattrapable proprement. → l'isolation couvre le `step` pur compilé et le glue Rust, **pas** un panic survenu à l'intérieur d'un appel FFI étranger arbitraire.
- BEAM isole par **heap séparé matériellement** ; `catch_unwind` isole le *flot de contrôle*, pas la mémoire — un `unsafe` corrupteur dans le glue peut encore nuire. En pratique le cœur llmlang étant safe Rust, le risque est faible mais **non nul**.

**L'argument d'union (peut-on ÉGALER voire BATTRE BEAM en stabilité réelle ?)** :

> Stabilité observée ≈ (pannes cœur) + (pannes frontière).
> - **Pannes cœur : ~0 par preuve.** Un `step` vérifié ne peut pas violer son contrat, ne peut pas diverger, ne peut pas casser son invariant. BEAM n'a pas cette garantie (un GenServer buggé plante). **Ici llmlang dépasse BEAM.**
> - **Pannes frontière : isolées + récupérables** *si* les deux changements ci-dessus sont en place *et* que le mode `unwind` est actif. Alors une panne d'effet = un acteur redémarré, pas le processus. **Parité avec BEAM.**

**Où l'argument NE tient PAS** (honnêteté requise) :
- Si `panic=abort` ou si le panic naît dans un FFI étranger → pas d'isolation → **en retrait de BEAM**, dont l'isolation est inconditionnelle.
- La classe de pannes qui **domine en production** (I/O qui échoue, ressource externe indisponible, FFI qui corrompt) est **précisément** la frontière non vérifiée. « Bugs de logique prouvés absents » est réel mais **ne réduit pas** cette classe-là. L'avantage de preuve est **asymétrique** : il brille là où BEAM n'avait de toute façon pas beaucoup de pannes (logique testée), et n'aide pas là où les pannes sont fréquentes (effets).

**Conclusion §6 :** llmlang peut atteindre la **parité d'isolation** avec BEAM sur la frontière (moyennant `unwind` + état possédé + `catch_unwind`) et **dépasser** BEAM sur le cœur (preuve). Il **reste en retrait** sur le cas FFI-étranger/`abort` et sur l'inconditionnalité. Net : *pour l'union*, plausiblement **égal ou légèrement supérieur** à BEAM en logique applicative, **pas** supérieur en robustesse face aux fautes matérielles/FFI brutes.

---

## 7. Replay déterministe des entrelacements (l'atout n°1 vs BEAM)

**Ce que CPT-LLL-014 revendique :** « traçabilité SUPÉRIEURE à l'observer BEAM » — rejouer l'entrelacement **exact** des messages.

**Pourquoi c'est plus tractable ici que dans le cas général** (le point original le plus fort du document) :
- Le record-replay concurrent général est notoirement dur parce qu'il faut capturer **tout** le non-déterminisme : ordonnancement OS, entrées, état interne partagé, timing. Chez nous, **deux réductions massives** :
  1. **On possède le scheduler.** On enregistre les **décisions d'ordonnancement à la source** (au point de livraison de chaque message), pas par instrumentation externe fragile. BEAM ne le peut pas — son observer regarde de l'extérieur et ne contrôle pas l'ordre.
  2. **Les comportements sont purs + totaux.** Un `step(state, msg)` est **déterministe** : mêmes entrées → même sortie, toujours (garanti par Z3). Il n'y a **aucun non-déterminisme interne par acteur** à capturer. Le **seul** non-déterminisme du système est **l'ordre de livraison des messages**. Rejouer = fixer cet ordre. C'est un espace de non-déterminisme **radicalement plus petit** que pour un acteur Elixir/Go arbitraire (qui peut faire de l'I/O, lire une horloge, générer de l'aléa au milieu d'un handler).

**Mécanisme concret proposé (extension du trace existant, pas un système parallèle) :**
- Le trace actuel (`src/codegen.rs` ~1961) est un `thread_local!` JSONL `{"eff":..,"v":..}` mono-thread. On l'étend en **trace de livraison** :
  - Un **compteur de séquence logique global monotone** (`AtomicU64`), incrémenté **au point de livraison** (quand le moteur retire un message d'une mailbox et l'applique). Chaque entrée de trace de livraison : `{"seq":N, "pid":P, "msg":M}` (et, avec ADT §9, l'encodage du message).
  - En **mode `--trace`** : le moteur écrit cette entrée à chaque livraison, dans un fichier **process-global** (pas thread-local — changement nécessaire, cf. ci-dessous).
  - En **mode `--replay`** : le moteur **ignore l'ordre d'arrivée réel** et livre les messages **strictement dans l'ordre `seq`** enregistré. Comme `step` est déterministe, l'état de chaque acteur est reconstruit à l'identique → entrelacement exact rejoué. Les effets IO à l'intérieur (via IO.print etc.) réutilisent le mécanisme `(eff, v)` existant, désormais indexé par `seq`.
- **Changement de fond nécessaire :** le trace doit passer de `thread_local!` à **process-global thread-safe** (un `Mutex<File>` d'écriture, ou un canal dédié vers un thread d'écriture unique pour éviter la contention). C'est un vrai travail mais **borné** — c'est une extension du format, pas une réinvention.
- ~~Aspérité existante à corriger d'abord : REQ-LLL-028...~~ **CORRIGÉ (2026-07-04) : déjà livré, vérifié empiriquement — pas un prérequis.** Voir la correction en §1.

**Difficulté honnête :**
- Enregistrer l'ordre de livraison **sérialise un point** (le compteur global au point de livraison) — surcoût sous `--trace`, mais `--trace` est un mode d'observation, pas le mode prod. Acceptable.
- Le vrai piège : si un jour un effet **introduit du non-déterminisme non tracé** (horloge, aléa, réseau) *à l'intérieur* d'un step, le replay diverge. Mitigation : ces effets sont **déjà** des effets extern tracés `(eff, v)` — tant que **tout** non-déterminisme passe par la frontière d'effet (invariant du langage), il est capturé. C'est précisément là que l'architecture voie 2a paie : *rien* n'échappe à la frontière, donc *tout* le non-déterminisme est traçable. **BEAM n'a pas cet invariant** (un processus peut appeler `os:timestamp()` n'importe où sans trace).

**Verdict §7 :** c'est l'axe où llmlang **dépasse réellement BEAM**, et l'affirmation est défendable *par construction*, pas par optimisme. À condition de (a) rendre le trace process-global, (b) stamper une séquence au point de livraison — REQ-LLL-028 n'est plus dans cette liste (déjà livré). Difficulté : réelle mais **bornée**, bien plus petite que le record-replay général grâce à la pureté+totalité du cœur.

---

## 8. Supervision / « let it crash » pour la frontière (pas le cœur)

**Que signifie « let it crash » quand les bugs de logique sont censés être pré-éliminés par preuve ?**
- Dans BEAM, « let it crash » couvre surtout des bugs de logique et des états inattendus. Chez llmlang, **le cœur ne peut pas crasher pour cause de logique** (prouvé). Donc « let it crash » ne s'applique **qu'à la frontière d'effet** : un `extern`/FFI qui échoue, une ressource externe absente, un panic dans le glue.
- Conséquence : la supervision llmlang est **plus étroite et plus ciblée** que celle de BEAM — elle n'a à gérer *que* les pannes d'effet, jamais les pannes de comportement. C'est un simplification, pas une lacune.

**Mécanisme proposé (W3, déclaré, non prouvé — DEC-LLL-017) :**
- Sur `catch_unwind` d'un `step` (§6), politique de redémarrage runtime **déclarée** :
  - **restart-fresh** (défaut) : réinitialiser l'état de l'acteur à sa valeur `spawn` initiale (analogue `one_for_one` + état connu-bon). Simple, sûr, aligné sur l'invariant « l'état initial satisfait `requires`/l'invariant ».
  - **restart-last-good** : conserver le dernier `state` qui a satisfait l'invariant avant le message fautif. Nécessite de garder ce snapshot (bon marché : `state` est un scalaire aujourd'hui, une valeur ADT demain).
  - **stop** : retirer l'acteur, notifier (analogue arrêt définitif).
- **Anti-tempête** (repris de BEAM) : intensité max déclarée (`MaxR` redémarrages en `MaxT`) au-delà de laquelle on escalade (arrêt du groupe d'acteurs, ou du superviseur). Empêche une boucle de crash-restart de brûler le CPU.
- **Pas d'arbre OTP complet en v1** : un niveau de supervision (le moteur supervise ses acteurs) suffit pour le tracer-bullet. Les stratégies `one_for_all`/`rest_for_one` (dépendances inter-acteurs) sont une évolution, pas un besoin initial.

**Honnêteté :** la supervision de slice-1+ sera **déclarée, pas prouvée** (elle vit au runtime, DEC-LLL-017). C'est correct et assumé — le cœur reste prouvé, la politique de récupération est un concern runtime typé mais non vérifié, exactement comme les effets.

---

## 9. Comportements génériques + messages ADT riches — **gaté sur REQ-LLL-052**

**À énoncer clairement pour le lecteur futur :** les restrictions « un seul `step` fixe » et « messages `Int` seulement » **ne sont pas** un choix d'architecture de scheduler — elles persistent **jusqu'à** résolution du marshalling de valeurs à la frontière FFI :

- **Comportement générique passé EN VALEUR à `spawn(behavior)`** exige de marshaller une **valeur fonctionnelle** (`fn(State, Msg) -> State`) à travers la frontière extern. Ce mécanisme **n'existe pas** (même famille de gap que REQ-LLL-052).
- **Messages ADT riches** (au lieu de `Int`) exigent de marshaller une **valeur ADT arbitraire** à travers la frontière. C'est **exactement REQ-LLL-052** (« FFI: general ADT/sum-type marshalling »), aujourd'hui limité au cas spécial `Result<T,E>` (`src/types.rs` ~860-876) ; pas de mécanisme général pour un type somme utilisateur arbitraire.

**REQ-LLL-052 est explicitement `needs-design-twice`, pas démarré, et touche la frontière de soundness (havoc/DEC-LLL-017).** Ce document **ne le re-conçoit pas** — il **marque la dépendance** : le tier-2 (§5, candidat B) est livrable **avec** la restriction v1 (un `step`, `msg: i64` dans les canaux Tokio), et les restrictions ne tombent **que** quand REQ-LLL-052 (+ son extension aux valeurs fonctionnelles) est résolu avec sign-off opérateur. **Ne pas coder le marshalling ADT/fonctionnel dans le cadre de W2.**

---

## 10. Distribution multi-nœuds (esquisse seulement — hors roadmap)

Pas sur la roadmap. Question posée : l'architecture frontière-d'effet **accommoderait-elle** la distribution plus tard ?

- **Oui, structurellement.** `spawn`/`send`/`state` sont déjà des effets extern opaques. Un Pid « local » (index dans la table `Pid → Sender`) deviendrait un Pid « global » (`{node, local_id}`) ; `send` router vers un transport réseau (comme `net_kernel`/`epmd` de BEAM) au lieu d'un canal local. Le cœur vérifié **ne changerait pas** (il ne voit que la signature havoc'd — c'est tout l'intérêt de voie 2a).
- **Le replay §7 se généralise mal** en distribué (horloge logique globale → horloges vectorielles/Lamport par nœud, réordonnancement causal). C'est le vrai coût caché ; à ne pas sous-estimer si la distribution devient un objectif.
- **Verdict :** l'architecture **n'exclut pas** la distribution (bon signe), mais celle-ci rouvrirait la question du replay déterministe (§7) à un niveau de difficulté supérieur. Ne rien concevoir maintenant.

---

## 11. Hot code reloading — verdict indépendant (ne PAS se contenter de l'intuition opérateur)

**Intuition opérateur à évaluer :** le hot-reload serait en tension profonde, peut-être irréconciliable, avec l'identité content-hash (DEC-LLL-020/034) + la vérification Z3 statique.

**Verdict indépendant : la tension est RÉELLE mais PLUS FAIBLE que supposé. Le hot-reload est compatible, et l'identité content-hash le rend même PLUS propre que celui de BEAM — pas moins.** Argumentaire précis :

- **L'identité content-hash n'interdit pas le reload — elle le rend PRÉCIS.** Dans BEAM, le hot-swap se fait *par nom de module* : l'ancienne et la nouvelle version partagent un nom, et le runtime jongle deux versions (`old`/`current`) d'un même nom — source classique de confusion (quel code tourne réellement ?). Chez llmlang, **un nouveau comportement = un nouveau hash = une identité distincte, sans ambiguïté**. « Recharger l'acteur X vers le comportement `hash_B` » est une opération *exacte et auditable* : on sait précisément quelle définition canonique tourne (DEC-LLL-034 : le graphe content-addressed est la surface de navigation). C'est un **avantage**, pas un obstacle.
- **La vérification statique ne bloque pas — elle se compose avec un swap atomique.** Le schéma viable :
  1. Le nouveau comportement `step_B` est **compilé + vérifié Z3** normalement (il termine, respecte son contrat) — comme n'importe quelle part. Rien de neuf.
  2. Une **fonction de migration `migrate: State_A -> State_B`** est fournie **et est elle-même une part totale vérifiée** (pré/postcondition : elle produit un `State_B` satisfaisant l'invariant de `step_B`). C'est le point clé : la migration d'état, danger n°1 du hot-reload, devient un **objet prouvé** — quelque chose que BEAM ne peut pas offrir (son `code_change/3` est du code non vérifié, source réputée de bugs de reload).
  3. **Swap atomique au point de livraison** : le moteur, entre deux messages (jamais au milieu d'un `step`), applique `state_B = migrate(state_A)` puis bascule l'acteur sur `step_B`. Atomique car `step` est court, borné, non-préemptible en son milieu (on contrôle le moteur).
- **La tension résiduelle, nommée précisément :** ce n'est **pas** identité vs reload, c'est **la nécessité d'une fonction de migration prouvée**. Si l'auteur (LLM) ne fournit pas de `migrate` valide (par ex. `State_B` n'a pas de mapping total depuis `State_A`), le reload est **refusé à la compilation** — cohérent avec l'invariant « obligation non déchargée = erreur, jamais de repli runtime » (DEC-LLL-015/017). C'est **plus strict** que BEAM (qui accepterait un `code_change` bancal et crasherait au runtime), donc **plus sûr**, au prix d'exiger la preuve de migration.
- **« Let it crash + restart-fresh » réduit le BESOIN de hot-reload.** Une grande partie de l'usage BEAM du hot-reload (corriger un état corrompu à chaud) est déjà couverte par restart-fresh (§8) : redémarrer l'acteur avec le nouveau comportement et un état initial propre est souvent suffisant, et **ne nécessite aucune migration**. Le hot-reload avec migration n'est requis que pour préserver un état long-vécu à travers un changement de comportement.

**Verdict §11 :** le hot-reload est **faisable et cohérent** avec l'identité content-hash et la vérification Z3 ; l'identité content-hash est un **atout** (swap sans ambiguïté de version) et la vérification transforme la migration d'état en **objet prouvé** (supérieur au `code_change` non vérifié de BEAM). La seule vraie contrainte — exiger une fonction de migration totale prouvée — est un durcissement, pas un blocage. **L'intuition « irréconciliable » est infirmée.** À noter : c'est de l'analyse de faisabilité, pas une priorité roadmap.

---

## 12. Synthèse — tableau verdict

| Axe | llmlang (cible) | vs BEAM | vs Tokio/Actix/Go |
|---|---|---|---|
| Débit / scheduling | Hérité de Tokio (work-steal, LIFO, coop 128) | ~ (BEAM comparable) | = (on **est** Tokio) |
| Isolation par acteur | État possédé + `catch_unwind` (§6) | ≈ parité si `unwind` ; < si `abort`/FFI | > Actix (tâche vs Arbiter) ; >> Go (Go = 0 isolation) |
| Bugs de logique | **Prouvés absents** (Z3) sur le cœur | **>** BEAM (partiel : cœur seul) | **>** tous (aucun ne prouve la logique) |
| Data races | Par construction runtime (état possédé, 0 partage) | ≈ | > Go (runtime-only) ; ≈ Pony (Pony = statique par types) |
| Replay entrelacements | **Séquence logique au point de livraison** (§7) | **>** BEAM (observer ne rejoue pas) | > (aucun ne le fait nativement) |
| Supervision | restart-fresh/last-good, anti-tempête (§8) | < BEAM (pas d'arbre OTP complet v1) | > Actix ; >> Go (aucune) |
| Généricité comportement / ADT | **Gaté REQ-LLL-052** (§9) | < (BEAM: dynamique) | < (limitation temporaire) |
| Distribution | Architecture compatible, non conçue (§10) | << BEAM | ~ |
| Hot-reload | Faisable, migration **prouvée** (§11) | migration > BEAM ; maturité < BEAM | > (rare ailleurs) |
| Maturité / durcissement | Neuf | **<<** BEAM (irréductible par doc) | < (Tokio/Go rodés) |

**Bottom line (reprise §0) :** oui pour égaler Rust/Go (en construisant dessus) ; dépassement plausible de BEAM sur **exactement deux axes** — classe de bugs prouvée absente (cœur) et **replay déterministe des entrelacements** (l'atout structurel décisif) ; retrait honnête et irréductible sur la maturité, la distribution, la largeur OTP, et l'asymétrie d'isolation (la frontière non vérifiée est là où vivent les vraies pannes).

---

## Annexe — décisions structurelles à trancher avant W2-t2 (pour l'opérateur)

1. **Moteur tier-2 : Tokio (recommandé) vs threads-OS-jalon.** Trancher si l'on veut le jalon d'isolation A avant l'échelle B, ou directement B.
2. **`panic = "unwind"` imposé** pour les modules à runtime d'acteurs (sinon `catch_unwind` inerte). Coût perf marginal accepté ?
3. **Trace process-global** (§7) : valider le passage `thread_local!` → `Mutex<File>`/thread d'écriture. (REQ-LLL-028 n'est PAS un prérequis — déjà livré, voir correction §1.)
4. **REQ-LLL-052 reste hors W2** : confirmer que tier-2 est livré avec la restriction v1 (un `step`, `msg: i64`).
5. Toutes ces décisions touchent le runtime, **aucune** ne touche `vc.rs` / la soundness du cœur (à re-confirmer par recherche avant code — tripwire slice-1).
