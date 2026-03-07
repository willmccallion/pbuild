# pbuild

A small, parallel build system written in Rust. Weekend project — built to understand how tools like Ninja work under the hood.

Rules are declared in a `pbuild.toml` file. pbuild hashes input files to decide what needs rebuilding, runs independent rules in parallel, and persists hashes to `.pbuild.lock` for fast incremental builds.

---

## Install

```sh
cargo install --path .
```

---

## Quick start

Create a `pbuild.toml` in your project root:

```toml
[config]
default = "app"

["main.o"]
command = ["cc", "-c", "main.c", "-o", "main.o"]
inputs  = ["main.c"]
output  = "main.o"

[app]
command = ["cc", "-o", "app", "main.o"]
deps    = ["main.o"]
inputs  = ["main.o"]
output  = "app"
```

Then run:

```sh
pbuild              # build the default target
pbuild app          # build a specific target
pbuild --list       # list all targets
pbuild why app      # explain why app would rebuild
pbuild clean        # delete outputs and reset the lock file
```

---

## pbuild.toml reference

```toml
[config]
default = "app"              # target to build when none is specified on the CLI
jobs    = 8                  # default parallelism (overridden by -j on the CLI)
env     = ["CC", "CFLAGS"]  # env vars that trigger a full rebuild when changed

["main.o"]
type    = "file"             # "file" (default) or "task"
command = ["cc", "-c", "main.c", "-o", "main.o"]
deps    = ["main.o"]         # targets that must be built first
inputs  = ["main.c"]         # files that trigger a rebuild when changed (globs supported)
output  = "main.o"           # file written by this rule (hashed after success)
depfile = "main.d"           # compiler-written depfile; pbuild injects -MF automatically
```

### `[config]`

| Key | Type | Description |
|-----|------|-------------|
| `default` | string | Target to build when none is given on the CLI |
| `jobs` | integer | Default parallelism; overridden by `-j` |
| `env` | string array | Environment variables that trigger a full rebuild when their value changes |

### Rule fields

| Field | Type | Description |
|-------|------|-------------|
| `type` | string | `"file"` (default) or `"task"` |
| `command` | string array | Command to run |
| `deps` | string array | Targets that must be built first |
| `inputs` | string array | Files to hash for dirty-checking; globs supported |
| `output` | string | File produced by this rule; hashed after success |
| `depfile` | string | Path where the compiler will write a depfile; pbuild injects `-MF <path>` automatically |

### `type`

- `file` — the rule produces an output file. pbuild hashes it after a successful build and skips the rule on the next run if nothing has changed.
- `task` — a phony target (like `test` or `lint`). Always runs if any of its deps were rebuilt.

### `inputs` and dirty checking

pbuild hashes every file listed in `inputs` before deciding whether to run a rule. If all hashes match `.pbuild.lock`, the rule is skipped. Glob patterns (`src/**/*.c`, `include/*.h`) are expanded at load time.

A rule with no `inputs` always runs.

### `depfile` — automatic header tracking

When `depfile = "foo.d"` is set, pbuild automatically appends `-MF foo.d` to the command. After the build, it parses the depfile and records every discovered header in `.pbuild.lock`. On future runs those headers are checked for changes — so modifying any included header triggers a rebuild, without listing every header manually in `inputs`.

```toml
["main.o"]
command = ["cc", "-c", "main.c", "-o", "main.o"]
inputs  = ["main.c"]
output  = "main.o"
depfile = "main.d"
```

### `env` tracking

Environment variables listed in `[config] env` are stored in `.pbuild.lock`. If any change between runs, every rule rebuilds. This catches the common mistake of changing `CC` or `CFLAGS` and getting a silently stale build.

---

## CLI reference

```
Usage: pbuild [OPTIONS] [TARGET]
       pbuild clean
       pbuild why <TARGET>

Options:
  -j <N>, --jobs <N>   Run at most N rules in parallel (default: logical CPUs)
  -n, --dry-run        Print commands without running them
  -k, --keep-going     Keep building independent rules after a failure
  -v, --verbose        Print skipped rules
  -l, --list           List all available targets and exit
  -h, --help           Print this help and exit

Special targets:
  clean                Delete all rule outputs and .pbuild.lock
  why <TARGET>         Explain why a target would rebuild
```

---

## How it works

1. `pbuild.toml` is parsed into a list of rules.
2. A dependency graph is built and topologically sorted.
3. Rules are executed in waves — all rules whose dependencies are satisfied run in parallel, bounded by `-j`.
4. After each wave, input and output files are hashed and flushed to `.pbuild.lock`.
5. On the next run, rules whose hashes haven't changed are skipped.

---

## License

MIT
