<div align="center">

# strata

A structured-data shell written from scratch in Rust. It is inspired by Nushell, but every part is original code: the lexer, a small parser-combinator library, the parser, a static type checker, a lazy streaming evaluator, 68 builtins and a table ...

![License](https://img.shields.io/badge/LICENSE-MIT-blue?style=for-the-badge)  ![Language](https://img.shields.io/badge/RUST-dea584?style=for-the-badge&logo=rust&logoColor=white)

Refer to [LICENSE](LICENSE) for the legal bits.

</div>

## 🚀 Quick start

```
strata                      # interactive REPL (rustyline, with history in ~/.strata_history)
strata -c '<pipeline>'      # run one command line
strata script.st            # run a script file
strata --check script.st    # parse and type-check only
echo 'ls | length' | strata # read the program from stdin
```

`STRATA_WIDTH` overrides the detected terminal width for table rendering.

### Language

```
ls | where size > 10kb | sort-by modified -r | first 5     # row conditions with bare column names
let xs = [1 2 3]; mut total = 0
for x in $xs { $total += $x }
def greet [name: string, --loud(-l), times?: int = 1] { if $loud { $"HI ($name)" } else { $"hi ($name)" } }
def sum-all [...nums: int]: nothing -> int { $nums | math sum }
$cfg.server.ports.0                                        # cell paths, `?` for optional members
(open data.json | get items) | each {|it| $it.price * 1.2 } | math avg
1..10 | reduce -f 0 {|x, acc| $acc + $x}
^git log --oneline | lines | first 3                       # externals, byte streams, auto-conversion
```

Values: int, float, bool, string, filesize (`10kb`, `2mib`), duration (`5sec`, `1hr`),
date, range (`1..10`, `0..<n`, `5..`), list, record, table (list of records), closure,
nothing, binary, error. Operators by precedence: `or` < `and` < `not` < comparisons
(`== != < <= > >= =~ !~ in not-in starts-with ends-with`) < `+ - ++` < `* / // mod` < `**`.

Builtins include: `ls ps open save cd pwd env help table echo describe`, `to json/csv/toml/text`,
`from json/csv/toml`, `where filter each reduce select reject get update insert rename`,
`sort sort-by group-by uniq reverse flatten first last skip take range length enumerate wrap append
prepend compact columns`, `math sum/avg/min/max/round`, `str upcase/downcase/trim/length/contains/replace/split/join`,
`lines split row split column`, `into int/float/string`, `is-empty ignore error make`.
`help` lists them; `help <cmd>` prints the signature and accepted input/output types.

## 🏗️ Architecture

```
 source ──► lexer.rs ──► tokens ──► parser.rs (built on combinator.rs) ──► AST (ast.rs)
                                         │  errors recovered per statement, all reported
                                         ▼
                              checker.rs: infer expression types, check args,
                              pipeline input/output against Signature.io (types.rs)
                                         │  type errors (with spans) stop execution
                                         ▼
                 eval.rs: Eval { Rc<Globals>, scopes }  ──►  PipelineData
                    │                                         ├─ Value(v)
                    │ builtins (commands.rs) are fns:          ├─ Stream(Box<dyn Iterator>)   ← lazy
                    │   (Eval, CallArgs, PipelineData)         └─ Bytes(child stdout reader)  ← externals
                    ▼
            engine.rs: print — strings raw, structured values via table.rs
                       (column fitting / truncation to terminal width)
```

* **combinator.rs** — parsers are `Fn(Input) -> Result<(Input, O), PErr>` over a token slice.
  Combinators: `token tok keyword map map_span and_then try_map alt opt many many1 sep_by pair
  preceded terminated delimited cut chainl1 recover`. Errors carry spans and a `committed` flag
  (`cut`), `alt` merges expectations at the same position ("expected integer or ','"),
  `delimited` reports unclosed delimiters at the opening token, and `recover` records the error
  in the shared context and resynchronises so one parse reports many errors.
* **parser.rs** — the grammar: statements (`let/mut/def/for/while/assignment`), pipelines, command
  calls resolved against the signature table (multi-word names like `str upcase`, flags with
  short forms, typed flag values), externals (`^cmd` or unknown names), precedence climbing via
  `chainl1`, closures/records/blocks, string interpolation (sub-lexed with absolute offsets so
  spans stay exact), cell paths, ranges, and a nesting-depth guard.
* **checker.rs** — gradual typing: `any` is compatible with everything; everything else is checked
  (operators, `if`/`while` conditions, argument types and arity, flag value types, declared
  `let x: T`, `def ...: in -> out` result types, cell paths on scalars, column access in `where`
  on scalar rows, and command input types, e.g. `"abc" | math sum`).
* **eval.rs** — pipelines are iterator chains. `range`, `each`, `where/filter`, `skip/take/first`,
  `enumerate`, `lines`, `compact`, `select/reject/update/insert/rename` on tables are lazy, so
  `range 1..1_000_000_000 | first 3` and `^yes | lines | first 2` finish immediately. Closures
  capture their environment by value; `ClosureRunner` lets lazy iterators call back into the
  evaluator. External commands get stdin from the previous value (strings verbatim, lists one per
  line, tables rendered) on a writer thread, and their stdout is a byte stream that internal
  commands auto-convert to a string (or read line by line for `lines`). Non-zero exit codes
  become errors.

## 🧪 Testing

`cargo test` runs everything (under 3 s once compiled):

| suite | what it covers | count |
|---|---|---|
| unit (`src/*`) | lexer tokens/spans, every combinator (incl. `cut`, `recover`, exact error spans), subtyping, table snapshots at widths 120/50/30, "every width 20..140 fits", wildcard globbing | 25 tests |
| `tests/parser.rs` | precedence/associativity, calls, flags, row conditions, defs, interpolation spans, **error recovery with exact spans** (multiple errors per parse, recovered statements kept), nesting limit | 11 tests |
| `tests/checker.rs` | 31 accepted programs, 37 rejected programs checked by message (6 also by exact span), type errors prevent side effects, REPL variable types | 7 tests |
| `tests/eval.rs` | arithmetic, overflow, scoping, closures, custom commands (defaults, rest, flags, recursion), cell-path update/insert, format round-trips, save/open, cd | 9 tests |
| `tests/golden.rs` | **244 `strata -c` pipelines** over json/csv/toml/txt fixtures, comparing stdout + stderr + exit code (`tests/golden/cases.txt`, regenerate with `BLESS=1`) | 1 test / 244 cases |
| `tests/streaming.rs` | laziness: 1e9 range, infinite ranges through `each`/`where`, infinite external (`yes`), pull-based library stream | 5 tests |
| `tests/external.rs` | external↔internal conversion in both directions, chained externals, 2 MB stdin without deadlock, exit codes, globbing | 6 tests |
| `tests/props.rs` | proptest: `parse(print(ast)) == ast` (span-insensitive) on random ASTs, formatting idempotence, parser+checker fuzzing on token soup (3000 cases) and arbitrary unicode | 4 properties |

Last full run: **68 tests passed, 0 failed, 0 ignored** (golden: 244/244 cases).
`cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` are clean.
`cargo check --target x86_64-pc-windows-gnu` succeeds (compile-only; not run on Windows).

The round-trip property found a real printer bug during development (an `if` expression used as
an operator operand was being re-parenthesised); the fuzzers found a stack overflow on deeply nested
parentheses, which is now a reported "nesting too deep" parse error.

## ⏱️ Benchmarks

`cargo bench` (release build, best of 3, single thread, on a shared 4-core VM):

```
range 1M | math sum                                   59.6 ms
range 1M | each {x*2} | where | length               706.7 ms
1M records: build | where col | length              1216.7 ms
range 1G | first 3 (laziness)                          0.0 ms
open 1M-row csv | where value > 500 | length        1275.5 ms   (parse 1M rows + filter)
1M-row table: get value | math avg                   785.3 ms
1M-row table: sort-by value | first                 1213.7 ms
1M-row table: group-by group | columns              1208.8 ms
```

Each closure call costs roughly 0.3–0.6 µs (a fresh scope per call). The table benchmarks are
dominated by cloning: `$t` copies the whole 1M-row table because values are not reference-counted.

## ⚠️ Honest limits

* Values are deep-copied (`Vec`-backed lists/records, no `Rc`), so referencing a large variable
  costs a full copy.
* Nushell's table literal `[[a b]; [1 2]]`, `match`, modules, `$env` assignment, background jobs,
  redirection (`out>`), and completions are not implemented.
* Command arguments are atoms (literals, variables, `( )`, lists, records, closures); write
  `echo (1 + 2)` rather than `echo 1 + 2`. `where` is the only command whose argument is a full
  expression (a row condition).
* Integer-only ranges; dates have no literal syntax (they come from `ls`, toml, or arithmetic).
* Glob expansion supports `*`/`?` in the last path component only.
* The type checker is gradual: anything typed `any` (e.g. `open` output, external output) is only
  checked at run time.
* Fuzzing uses proptest-generated inputs, not coverage-guided libFuzzer (`cargo fuzz` needs nightly).
* Windows: compile-checked only. External tests are `#[cfg(unix)]`.

## Licence

MIT
