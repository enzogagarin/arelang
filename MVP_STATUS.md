# Arelang HTTP MVP Status

Status: complete as of 2026-05-27.

The HTTP MVP is the first working Arelang backend loop. It is intentionally not a full general-purpose language yet. It proves that Arelang can parse, check, run, inspect, test, audit, and document a real backend service through its own syntax.

## Completion Gate

The MVP is considered healthy when this command passes from the repository root:

```sh
./scripts/mvp-smoke.sh
```

The smoke gate verifies:

- Rust formatting, workspace tests, and clippy with warnings denied
- Arelang formatting for bundled examples
- `are check --json` diagnostics for `hello_api` and `users_api`
- `are inspect --json` contract manifest output
- `are openapi` export and drift checking
- `are audit --json` capability and route-contract checks
- `are test --json` built-in HTTP scenarios
- generated minimal and users-template projects
- real local HTTP server startup and curl-based response checks

## Current MVP Capabilities

- `are new` creates runnable Arelang HTTP projects
- `are run` starts local HTTP servers from checked Arelang source
- `are check` lexes, parses, resolves, and typechecks the MVP backend surface
- `are fmt` enforces canonical MVP syntax
- `are inspect` exposes the checked service contract manifest
- `are openapi` exports OpenAPI 3.1 JSON from the checked contract
- `are audit` checks the first least-privilege backend capability surface
- `are test` runs built-in HTTP scenarios without opening a TCP listener
- service routes support typed path params, body, query, headers, cookies, response type, and success status contracts
- route handlers can receive typed contract parameters directly
- successful domain payloads are wrapped into HTTP responses by route contracts
- `Result<T, E>` and `?` provide the MVP error flow
- `Http.error_map` maps typed domain errors to HTTP responses
- `model` declarations back the MVP in-memory `ctx.db.<collection>.insert/get` store
- declarative alias and field validations run at the HTTP boundary and are exported through `inspect` and OpenAPI

## Representative MVP Syntax

```are
type Email = opaque String validate.email
type DisplayName = opaque String validate.length(min: 2, max: 80)
type UserId = opaque U64

struct CreateUserInput {
    email: Email
    name: DisplayName
}

model User collection users {
    id: UserId primary
    email: Email unique
    name: DisplayName
}

fn create_user(ctx: Http.Context<AppState>, input: CreateUserInput) -> Result<User, ApiError> {
    let user = ctx.db.users.insert(input)?
    return user
}

service UsersApi(state: AppState) {
    use Http.error_map(map_error)

    post "/users" body CreateUserInput -> create_user returns User status 201
    get "/users/{id: UserId}" -> get_user returns User status 200
}
```

## Post-MVP Boundaries

These are intentionally outside the closed HTTP MVP:

- native codegen
- full package/module system
- real database adapters and migrations
- richer collection/query APIs
- request-scope safety checking
- full sandbox runtime
- package registry
- self-hosting
