#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/arelang-mvp.XXXXXX")"
PIDS=()
LAST_LOG_FILE=""

cleanup() {
    for pid in "${PIDS[@]:-}"; do
        kill "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
    done
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT INT TERM

log() {
    printf '\n==> %s\n' "$1"
}

run() {
    printf '+'
    printf ' %q' "$@"
    printf '\n'
    "$@"
}

assert_eq() {
    local actual="$1"
    local expected="$2"
    local label="$3"

    if [[ "$actual" != "$expected" ]]; then
        printf 'expected %s to be %q, got %q\n' "$label" "$expected" "$actual" >&2
        exit 1
    fi
}

assert_file_contains() {
    local file="$1"
    local needle="$2"
    local label="$3"

    if ! grep -Fq "$needle" "$file"; then
        printf 'expected %s to contain %q\n' "$label" "$needle" >&2
        printf '%s contents:\n' "$file" >&2
        cat "$file" >&2
        exit 1
    fi
}

wait_for_http() {
    local url="$1"
    local log_file="$2"

    for _ in $(seq 1 80); do
        if curl -fsS "$url" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.1
    done

    printf 'server did not become ready at %s\n' "$url" >&2
    printf 'server log:\n' >&2
    cat "$log_file" >&2
    exit 1
}

start_server() {
    local name="$1"
    local project="$2"
    LAST_LOG_FILE="$TMP_DIR/$name.log"

    run "$ROOT_DIR/are" run "$project" >"$LAST_LOG_FILE" 2>&1 &
    PIDS+=("$!")
}

assert_users_api_flow() {
    local base_url="$1"
    local label="$2"

    curl -fsS "$base_url/health" >"$TMP_DIR/${label}_health.json"
    assert_file_contains "$TMP_DIR/${label}_health.json" '"status":"ok"' "$label health response"

    local invalid_status
    invalid_status="$(
        curl -sS -o "$TMP_DIR/${label}_invalid_email.json" -w '%{http_code}' \
            -X POST "$base_url/users" \
            -H 'content-type: application/json' \
            -d '{"email":"invalid","name":"Ada"}'
    )"
    assert_eq "$invalid_status" "400" "$label invalid email HTTP status"
    assert_file_contains "$TMP_DIR/${label}_invalid_email.json" '"error":"invalid_email"' "$label invalid email response"

    local created_status
    created_status="$(
        curl -sS -o "$TMP_DIR/${label}_created_user.json" -w '%{http_code}' \
            -X POST "$base_url/users" \
            -H 'content-type: application/json' \
            -d '{"email":"ada@example.com","name":"Ada Lovelace"}'
    )"
    assert_eq "$created_status" "201" "$label create user HTTP status"
    assert_file_contains "$TMP_DIR/${label}_created_user.json" '"id":1' "$label create user response"
    assert_file_contains "$TMP_DIR/${label}_created_user.json" '"email":"ada@example.com"' "$label create user response"

    local list_status
    list_status="$(curl -sS -o "$TMP_DIR/${label}_list_users.json" -w '%{http_code}' "$base_url/users")"
    assert_eq "$list_status" "200" "$label list users HTTP status"
    assert_file_contains "$TMP_DIR/${label}_list_users.json" '"email":"ada@example.com"' "$label list users response"
    assert_file_contains "$TMP_DIR/${label}_list_users.json" '"name":"Ada Lovelace"' "$label list users response"

    local search_status
    search_status="$(curl -sS -o "$TMP_DIR/${label}_search_user.json" -w '%{http_code}' "$base_url/users/search?email=ada%40example.com")"
    assert_eq "$search_status" "200" "$label search users HTTP status"
    assert_file_contains "$TMP_DIR/${label}_search_user.json" '"email":"ada@example.com"' "$label search users response"

    local auth_status
    auth_status="$(
        curl -sS -o "$TMP_DIR/${label}_auth_check.json" -w '%{http_code}' \
            "$base_url/users/auth-check" \
            -H 'authorization: Bearer dev-token'
    )"
    assert_eq "$auth_status" "200" "$label auth check HTTP status"
    assert_file_contains "$TMP_DIR/${label}_auth_check.json" '"authorized":true' "$label auth check response"

    local session_status
    session_status="$(
        curl -sS -o "$TMP_DIR/${label}_session.json" -w '%{http_code}' \
            "$base_url/session" \
            -H 'Cookie: session_id=session-dev-123'
    )"
    assert_eq "$session_status" "200" "$label session HTTP status"
    assert_file_contains "$TMP_DIR/${label}_session.json" '"session_id":"session-dev-123"' "$label session response"
    assert_file_contains "$TMP_DIR/${label}_session.json" '"active":true' "$label session response"

    local get_status
    get_status="$(curl -sS -o "$TMP_DIR/${label}_get_user.json" -w '%{http_code}' "$base_url/users/1")"
    assert_eq "$get_status" "200" "$label get user HTTP status"
    assert_file_contains "$TMP_DIR/${label}_get_user.json" '"name":"Ada Lovelace"' "$label get user response"

    local missing_status
    missing_status="$(curl -sS -o "$TMP_DIR/${label}_missing_user.json" -w '%{http_code}' "$base_url/users/999")"
    assert_eq "$missing_status" "404" "$label missing user HTTP status"
    assert_file_contains "$TMP_DIR/${label}_missing_user.json" '"error":"not_found"' "$label missing user response"
}

log "format, tests, and lints"
run cargo fmt --all -- --check
run cargo test --workspace
run cargo clippy --workspace --all-targets -- -D warnings

log "static checks for bundled examples"
run "$ROOT_DIR/are" fmt "$ROOT_DIR/examples/hello_api" --check
run "$ROOT_DIR/are" fmt "$ROOT_DIR/examples/users_api" --check
run "$ROOT_DIR/are" check "$ROOT_DIR/examples/hello_api" --json
run "$ROOT_DIR/are" check "$ROOT_DIR/examples/users_api" --json
run "$ROOT_DIR/are" inspect "$ROOT_DIR/examples/hello_api" --json
run "$ROOT_DIR/are" inspect "$ROOT_DIR/examples/users_api" --json
run "$ROOT_DIR/are" inspect "$ROOT_DIR/examples/users_api" --json >"$TMP_DIR/users_inspect.json"
assert_file_contains "$TMP_DIR/users_inspect.json" '"schemas"' "users inspect contract"
assert_file_contains "$TMP_DIR/users_inspect.json" '"collection": "users"' "users inspect contract"
assert_file_contains "$TMP_DIR/users_inspect.json" '"primary": true' "users inspect contract"
assert_file_contains "$TMP_DIR/users_inspect.json" '"error_contract": "ApiError"' "users inspect contract"
assert_file_contains "$TMP_DIR/users_inspect.json" '"error_type": "ApiError"' "users inspect contract"
assert_file_contains "$TMP_DIR/users_inspect.json" '"response_type": "List<User>"' "users inspect contract"
run "$ROOT_DIR/are" openapi "$ROOT_DIR/examples/users_api" --output "$TMP_DIR/users_openapi.json"
assert_file_contains "$TMP_DIR/users_openapi.json" '"openapi": "3.1.0"' "users OpenAPI document"
assert_file_contains "$TMP_DIR/users_openapi.json" '"/users/{id}"' "users OpenAPI document"
assert_file_contains "$TMP_DIR/users_openapi.json" '"type": "array"' "users OpenAPI document"
assert_file_contains "$TMP_DIR/users_openapi.json" '"/users/search"' "users OpenAPI document"
assert_file_contains "$TMP_DIR/users_openapi.json" '"in": "query"' "users OpenAPI document"
assert_file_contains "$TMP_DIR/users_openapi.json" '"/users/auth-check"' "users OpenAPI document"
assert_file_contains "$TMP_DIR/users_openapi.json" '"in": "header"' "users OpenAPI document"
assert_file_contains "$TMP_DIR/users_openapi.json" '"/session"' "users OpenAPI document"
assert_file_contains "$TMP_DIR/users_openapi.json" '"in": "cookie"' "users OpenAPI document"
assert_file_contains "$TMP_DIR/users_openapi.json" '"#/components/schemas/User"' "users OpenAPI document"
assert_file_contains "$TMP_DIR/users_openapi.json" '"x-are-status": 404' "users OpenAPI document"
assert_file_contains "$TMP_DIR/users_openapi.json" '"const": "not_found"' "users OpenAPI document"
run "$ROOT_DIR/are" openapi "$ROOT_DIR/examples/users_api" --check --output "$TMP_DIR/users_openapi.json"
run "$ROOT_DIR/are" audit "$ROOT_DIR/examples/hello_api" --json
run "$ROOT_DIR/are" audit "$ROOT_DIR/examples/users_api" --json
run "$ROOT_DIR/are" test "$ROOT_DIR/examples/hello_api" --json
run "$ROOT_DIR/are" test "$ROOT_DIR/examples/users_api" --json

log "generated project smoke"
GENERATED="$TMP_DIR/generated_api"
run "$ROOT_DIR/are" new "$GENERATED" --name generated-api --port 18092
run "$ROOT_DIR/are" fmt "$GENERATED" --check
run "$ROOT_DIR/are" check "$GENERATED" --json
run "$ROOT_DIR/are" inspect "$GENERATED" --json
run "$ROOT_DIR/are" openapi "$GENERATED" --output "$GENERATED/openapi.json"
run "$ROOT_DIR/are" openapi "$GENERATED" --check
run "$ROOT_DIR/are" audit "$GENERATED" --json
run "$ROOT_DIR/are" test "$GENERATED" --json
start_server generated "$GENERATED"
generated_log="$LAST_LOG_FILE"
wait_for_http "http://127.0.0.1:18092/ping" "$generated_log"
curl -fsS "http://127.0.0.1:18092/ping" >"$TMP_DIR/generated_ping.json"
assert_file_contains "$TMP_DIR/generated_ping.json" '"message":"pong"' "generated ping response"

log "generated users template smoke"
GENERATED_USERS="$TMP_DIR/generated_users_api"
run "$ROOT_DIR/are" new "$GENERATED_USERS" --name generated-users-api --template users --port 18094
run "$ROOT_DIR/are" fmt "$GENERATED_USERS" --check
run "$ROOT_DIR/are" check "$GENERATED_USERS" --json
run "$ROOT_DIR/are" inspect "$GENERATED_USERS" --json
run "$ROOT_DIR/are" openapi "$GENERATED_USERS" --output "$GENERATED_USERS/openapi.json"
run "$ROOT_DIR/are" openapi "$GENERATED_USERS" --check
run "$ROOT_DIR/are" audit "$GENERATED_USERS" --json
run "$ROOT_DIR/are" test "$GENERATED_USERS" --json
start_server generated_users "$GENERATED_USERS"
generated_users_log="$LAST_LOG_FILE"
wait_for_http "http://127.0.0.1:18094/health" "$generated_users_log"
assert_users_api_flow "http://127.0.0.1:18094" "generated_users"

log "diagnostic UX smoke"
BROKEN_USERS="$TMP_DIR/broken_users_api"
cp -R "$GENERATED_USERS" "$BROKEN_USERS"
sed 's/post "\/users" body CreateUserInput -> create_user/post "\/users" body CreateUserInput -> create_usr/' \
    "$BROKEN_USERS/main.are" >"$BROKEN_USERS/main.are.tmp"
mv "$BROKEN_USERS/main.are.tmp" "$BROKEN_USERS/main.are"

if "$ROOT_DIR/are" check "$BROKEN_USERS" >"$TMP_DIR/broken_check.txt" 2>&1; then
    printf 'expected broken users API check to fail\n' >&2
    exit 1
fi
assert_file_contains "$TMP_DIR/broken_check.txt" 'error[E_RESOLVE_0002]: unknown route handler `create_usr`' "broken check diagnostic"
assert_file_contains "$TMP_DIR/broken_check.txt" 'post "/users" body CreateUserInput -> create_usr' "broken check source snippet"
assert_file_contains "$TMP_DIR/broken_check.txt" 'help: did you mean `create_user`?' "broken check suggestion"

log "users API HTTP smoke"
USERS_API="$TMP_DIR/users_api"
cp -R "$ROOT_DIR/examples/users_api" "$USERS_API"
sed 's/port = 8080/port = 18093/; s/127\.0\.0\.1:8080/127.0.0.1:18093/' \
    "$USERS_API/are.toml" >"$USERS_API/are.toml.tmp"
mv "$USERS_API/are.toml.tmp" "$USERS_API/are.toml"

start_server users "$USERS_API"
users_log="$LAST_LOG_FILE"
wait_for_http "http://127.0.0.1:18093/health" "$users_log"
assert_users_api_flow "http://127.0.0.1:18093" "example_users"

log "MVP smoke passed"
