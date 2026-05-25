# Compiler Architecture Seed

Arelang starts as an interpreter-first compiler toolchain. The first mission is a working HTTP server, not optimized native codegen.

## Bootstrap Language

Rust.

Reasons:

- strong enum and pattern matching support for AST and IR
- safe compiler internals
- good test tooling
- good fit for diagnostics and future region analysis

## Initial Crates

```text
crates/
  are-ast/           shared syntax tree data model
  are-cli/           CLI entry point; owns commands like check, run, fmt
  are-diagnostics/   human and JSON diagnostic data model
  are-lexer/         source text to token stream
  are-parser/        token stream to top-level AST
  are-project/       manifest loading and reusable static check pipeline
  are-resolver/      top-level symbol binding and service route checks
  are-typecheck/     type arity, duplicate fields, and HTTP service contract checks
  are-http-runtime/  first HTTP MVP server for checked service projects
```

Next crates:

```text
crates/
  are-runtime/
```

## First Pass Pipeline

```text
source.are
  -> lexer
  -> parser
  -> resolver
  -> type checker
  -> service registry
  -> interpreter/runtime
  -> HTTP server
```

## CLI Contract

```sh
are check examples/users_api
are check --json examples/users_api
are run examples/users_api
```

`are check --json` is the AI-agent contract. Its schema must remain stable once parser diagnostics begin.

Current `are check` behavior:

- collect `.are` files
- lex each file
- parse top-level items into AST
- resolve imports, declarations, service uses, and route handlers
- typecheck function signatures, generic arity, route handlers, and HTTP error mappers
- emit human or JSON diagnostics

Current `are run` behavior:

- load `are.toml`
- require `target = "server"`
- run static checks
- extract the checked service route registry
- start the users API HTTP MVP adapter

## Diagnostic Shape

Diagnostics should include:

- code
- severity
- file
- range
- problem
- reason
- fixes

Security-impacting fixes should not be automatically applied.
