# Arelang Syntax Seed

This is the first syntax seed for the HTTP MVP. It is intentionally small.

## Files

- Source files use `.are`
- Package manifests use `are.toml`
- Formatting is canonical and will be handled by `are fmt`

## Style

- Brace-based blocks
- No semicolons
- Newlines separate statements
- Public APIs must write types explicitly
- Local variables may use inference
- Mutation must be visible

```are
let name = "Ada"
let mut count = 0
```

## Functions

```are
fn health(ctx: Http.Context<AppState>) -> HealthResponse {
    return { "status": "ok" }
}
```

Functions use explicit return types. A missing return type means `Unit` only if the function is private and the compiler can prove it.

## Errors

v0 uses explicit `Result<T, E>`.

```are
fn create_user(ctx: Http.Context<AppState>, input: CreateUserInput) -> Result<User, ApiError> {
    let user = ctx.db.users.insert(input)?
    return user
}
```

`?` may only be used inside a function returning `Result`.

## Absence

v0 uses explicit `Option<T>`.

```are
fn find_user(id: UserId) -> Option<User> {
    return users.get(id)
}
```

Null is not part of the language.

## Structs

```are
type Email = opaque String validate.email
type DisplayName = opaque String validate.length(min: 2, max: 80)

struct User {
    id: UserId
    email: Email
    name: DisplayName
}
```

Domain aliases can carry validation contracts when a primitive has business meaning. Field-level validations remain available for one-off payload rules, but reusable concepts should be named as types.

## Enums

```are
enum ApiError {
    InvalidInput(message: String)
    NotFound
    Internal(message: String)
}
```

## Services And Routes

`service` and `route` are allowed in v0 because HTTP is the first product target.

```are
service UsersApi(state: AppState) {
    get "/health" -> health returns HealthResponse status 200
    post "/users" body CreateUserInput -> create_user returns User status 201
    get "/users/search" query SearchUsersQuery -> search_users returns SearchUsersResponse status 200
    get "/users/auth-check" headers AuthHeaders -> auth_check returns AuthCheckResponse status 200
    get "/session" cookies SessionCookies -> current_session returns SessionResponse status 200
    get "/users/{id: UserId}" -> get_user returns User status 200
}
```

The compiler builds a route registry from this declaration. Method shorthand is the canonical style, while the older `route GET "/path" -> handler` shape remains parseable during the MVP transition. Body contracts, query contracts, headers contracts, cookies contracts, response contracts, status contracts, and typed path parameters are checked against handler code: `body CreateUserInput` must line up with a handler param such as `input: CreateUserInput`, `query SearchUsersQuery` must line up with `query: SearchUsersQuery`, `headers AuthHeaders` must line up with `headers: AuthHeaders`, `cookies SessionCookies` must line up with `cookies: SessionCookies`, `returns User status 201` must line up with the success response where the compiler can infer it, and `{id: UserId}` must line up with `id: UserId`. Lower-level `req.json<T>()`, `req.query<T>()`, `req.headers<T>()`, `req.cookies<T>()`, and `ctx.param<T>()` remain available as compatibility escape hatches.

## v0 Keywords

```text
use
as
pub
fn
let
mut
struct
enum
type
opaque
if
else
while
for
in
return
break
continue
match
service
route
test
unsafe
foreign
true
false
```

New keywords should be rare. If a feature can start as a standard library API, it should start there.
