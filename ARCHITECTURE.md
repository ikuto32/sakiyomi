# Sakiyomi architecture

Sakiyomi is split into small crates with one direction of dependency. Pure data and format code
never touches processes, the network or global state; anything stateful lives in the app layer.

```
            shogi-core (rules: legal moves, SFEN, USI moves)        ← from shogi_GPU_AI
                 │
           sakiyomi-kifu   positions, moves, game records, all text/binary formats
            │         │
   sakiyomi-book   sakiyomi-usi   books (.db/.sbk/Gikou), merge/filter │ engine processes, protocol, registry
            │         │
            └── sakiyomi-app ──┘  sessions (games, analysis), tool jobs (convert, mine), localisation
                 │         │
          src-tauri (GUI)  sakiyomi-cli (`sakiyomi-tool`)
```

| Crate | Owns | Must not |
|---|---|---|
| `spg` | SPG v1.0 reference implementation: positions/CPE/keys, move legality (§6.7), the `.spg` container with full verification, engine-side lookup, `.spgt`, `.spgm` file sets with BLK3 and signatures | depend on any other Sakiyomi crate |
| `sakiyomi-spg` | conversions between SPG and `GameRecord` / `Book` (spec §10), `Board` ↔ SPG `Position`, Move16 drop-code mapping | contain container code |
| `sakiyomi-kifu` | `Position`, `Move`, `GameRecord` tree, KIF/KI2/CSA/JKF/SFEN/USI, evaluation comments, position codecs (SFEN, BOD, CSA, HuffmanCodedPos, PackedSfen), game databases (hcpe, hcpe3, PackedSfenValue, SFEN lines, CSA multi-game) | spawn processes, read env, panic on input |
| `sakiyomi-book` | `Book` model, YaneuraOu `.db`, ShogiGUI `.sbk`, Gikou `book.bin` (with Gikou's Zobrist keys), merge / filter / stats, the book miner algorithm (engine behind a trait) | know about Tauri or concrete engines |
| `sakiyomi-usi` | USI process, handshake, `go`/`info` parsing, engine registry | interpret games |
| `sakiyomi-app` | game/analysis sessions, clocks, `tools` (one function per conversion / mining operation), `jobs` (background threads with progress, cancellation, panic isolation), `books` (opened books for the book panel), the USI `Searcher` for mining, message localisation | contain format code |
| `sakiyomi-cli` | command-line front end for conversions and mining | contain logic beyond argument handling |
| `src-tauri` | Tauri commands/events, CPU/GPU monitor | contain logic beyond plumbing |

## Data model

* **Position**: shogi-core `Board` (squares, hands, side). Positions in books are keyed by
  SFEN *without* the move number, so transpositions share one entry regardless of ply.
* **GameRecord**: a tree of nodes. The first child is the main line. Each node holds the move,
  a typed `Special` terminal (resign, sennichite, …), comment, time, and `NodeAnalysis`:
  separate evaluations from the Sente engine, the Gote engine and the analysis engine, so the
  evaluation graph can draw both sides' readings.
* **Evaluation** values are always stored from Sente's point of view; perspective is converted
  once at the boundary (engine output, file formats that store side-to-move scores).

## SPG as the standard format

* Sessions save and open `.spg` / `.spgt` through `sakiyomi-app::spgio`. Book, record and database
  tools accept SPG on either side, and the book panel opens SPG books.
* `GameRecord` holds less than SPG. `sakiyomi_spg::spg_to_records` returns a `Lost` summary,
  which the GUI shows as a notice when a file is opened (spec §10.5: never drop data silently).
* A game engine's reading is stored on the position it thought about (the parent node), with its PV.
  When the PV does not start with the move played, `x.sakiyomi.played` names that move. An analysis is
  stored on the node itself.
* A book merge into SPG writes SRCS, PROV and per-item rights by default
  (`sakiyomi_spg::merged_books_to_spg`); asking for a merge is not the user's instruction to omit PROV
  (§8.4), so only `BookConvertRequest::no_prov` (`--no-prov`) leaves it out.
* Evaluations whose viewpoint comes from a comment convention (`EngineEval::assumed_viewpoint`) are written
  as "viewpoint unknown" with the value as written.

## Adding things

* **A file format**: a module in `sakiyomi-kifu` (records, positions, databases) or `sakiyomi-book`
  (books), with a reader that takes `Limits`, a writer, a round-trip test and a case in the
  randomised hostile-input tests.
* **A tool**: a request struct and a function in `sakiyomi-app::tools` that takes a cancel flag and a
  progress callback; then one `job_*` command in `src-tauri/src/main.rs`, one subcommand in
  `crates/cli`, and a form in `src/components/Tools.tsx`.
* **A view**: a `ViewId` in `src/layout.ts` (with a place in each preset and in `restoreView`) and a
  case in `Pane.tsx`'s `renderView`.
* **A UI string**: write the Japanese text in `t("…")` and add the English to `EN` in `src/i18n.ts`
  (`npm run test:i18n` lists what is missing). Backend messages go through `sakiyomi_app::i18n`.

## Background work

Conversions, book mining and opening large books run as jobs (`sakiyomi_app::jobs`): a thread per
job, progress events throttled to about ten per second (`shogi://job`), a shared cancel flag, and
`catch_unwind` so a bug in one job cannot take the application down. Mining engines watch the cancel
flag during a search and send `stop`; on exit the app cancels all jobs, waits briefly for them, then
closes every session so no engine process outlives the GUI.

## Security and robustness rules

These apply to every reader, because kifu and book files come from strangers on the internet.

1. **No panics on input.** No `unwrap`/indexing on data-derived offsets. Text is sliced only on
   char boundaries. Every parser has a randomised "never panics" test.
2. **Bounded resources.** `Limits` caps input size, line length, node count, variation depth,
   book positions and records per file; allocations never trust a length field in the file.
   Writers are iterative (no recursion depth driven by input).
3. **Typed errors with locations.** Parsers return `Error { kind, line }`; the app localises them.
4. **Engines** run only from a path the user chose, with no shell and no arguments, their output
   is length-limited per line, and every process is killed when its session ends.
5. **GUI**: strict CSP (`default-src 'self'`, `script-src 'self'`, no remote origins, `object-src 'none'`),
   a frozen JS prototype, minimal Tauri capabilities, text rendered as text (never as HTML).
6. **Files are written atomically** (temp file + rename), and the CLI refuses to overwrite
   unless `--force` is given.
