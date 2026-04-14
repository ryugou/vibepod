# Node.js + TypeScript Implementation (vibepod)

## Scope

- Runtime: Node.js. Bun and Deno are out of scope for v1.6 — use a custom template.
- Language: TypeScript (strict). JavaScript-only projects may use this bundle but the opinion will apply less cleanly.
- Framework: none. Next/Nest/Nuxt opinions are NOT in this bundle — use a custom template for framework-specific workflows.

## Rules (universal + Node/TS-specific)

- Write tests before implementation. Follow `skill: tdd-workflow`.
- `@ts-ignore` / `@ts-expect-error` require a justification comment explaining why the type system cannot express the constraint.
- Do not throw strings or plain objects. Throw `Error` (or a subclass) so stack traces are preserved.
- Do not leave unhandled promise rejections. Use `.catch(...)` at the boundary, or propagate via `await` inside an `async` function with a try/catch at the top-level handler.
- No stray `console.log` / `console.debug` in committed code. Use a structured logger (`pino`, `winston`, or project-specific equivalent) with appropriate level.
- No hardcoded secrets.
- Before declaring work complete: `tsc --noEmit` must be clean, project lint (`eslint` / `biome` / etc.) must pass, and tests must pass.

## Agents (prefer in this order)

- `typescript-reviewer` — TypeScript-specific review (type soundness, strictness violations, `any` audit, generic design).
- `code-reviewer` — generic review for cross-cutting concerns (API shape, naming, error handling, documentation).
- `code-architect` — design / planning before implementation.
- `code-explorer` — tracing existing code paths in an unfamiliar codebase.
- `silent-failure-hunter` — before commit, sweep for swallowed errors and bad fallbacks (`.catch(() => {})`, `try { ... } catch {}`).

## Skills (load as needed)

- `tdd-workflow` — strict RED → GREEN → REFACTOR discipline with git checkpoints.
