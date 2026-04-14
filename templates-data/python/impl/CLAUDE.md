# Python Implementation (vibepod)

## Scope

- Language: Python 3.11+. Earlier versions are out of scope for v1.6.
- Framework: none. Django / Flask / FastAPI opinions are NOT in this bundle — use a custom template for framework-specific workflows.

## Rules (universal + Python-specific universal bugs)

- Write tests before implementation. Follow `skill: tdd-workflow`.
- No bare `except:` or `except BaseException:`. Catch specific exception classes; re-raise with context (`raise NewError(...) from e`) rather than swallowing.
- No mutable default arguments (`def f(x=[])` / `def f(x={})`). Use `None` as the sentinel and construct inside the function. This is a universal Python bug.
- Do not use `assert` for runtime validation or security checks. `assert` is stripped under `python -O`, leaving the check absent in optimized builds. Use explicit `if not cond: raise ValueError(...)` instead. Tests are the exception.
- Do not apply `eval`, `exec`, `pickle.loads`, `yaml.unsafe_load`, or `subprocess(..., shell=True)` to untrusted input.
- No stray `print` / `pprint.pprint` / `breakpoint()` left over from debugging. User-facing CLI output via `print` is fine; diagnostics and logs should go through the `logging` module (or a project-specific logger) with appropriate level.
- No hardcoded secrets.
- Before declaring work complete: project's formatter (Black/Ruff format/autopep8), linter (Ruff/Flake8/Pylint), type checker (mypy/pyright), and tests must all pass. The exact tools are project-specific; vibepod enforces the discipline of running them all, not the choice.

## Agents (prefer in this order)

- `python-reviewer` — Python-specific review (type hints, context managers, async, iterator/generator discipline, dataclass / Pydantic usage).
- `code-architect` — design / planning before implementation.
- `code-explorer` — tracing existing code paths in an unfamiliar codebase.
- `silent-failure-hunter` — before commit, sweep for swallowed errors (`except Exception: pass`, bare `except`, `.get()` with silent fallback).
- `code-reviewer` — generic fallback for cross-cutting concerns (API shape, naming, comments) when the python-specific reviewer has nothing to add.

## Skills (load as needed)

- `python-patterns` — idiomatic Python (context managers, dataclasses, protocols, typing).
- `python-testing` — pytest conventions, fixtures, parametrize, mocking, coverage.
- `tdd-workflow` — strict RED → GREEN → REFACTOR discipline with git checkpoints.
