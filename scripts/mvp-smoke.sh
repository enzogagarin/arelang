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

log "format, tests, and lints"
run cargo fmt --all -- --check
run cargo test --workspace
run cargo clippy --workspace --all-targets -- -D warnings

log "static checks for bundled examples"
run "$ROOT_DIR/are" check "$ROOT_DIR/examples/hello_api" --json
run "$ROOT_DIR/are" check "$ROOT_DIR/examples/users_api" --json

log "generated project smoke"
GENERATED="$TMP_DIR/generated_api"
run "$ROOT_DIR/are" new "$GENERATED" --name generated-api --port 18092
run "$ROOT_DIR/are" check "$GENERATED" --json
start_server generated "$GENERATED"
generated_log="$LAST_LOG_FILE"
wait_for_http "http://127.0.0.1:18092/ping" "$generated_log"
curl -fsS "http://127.0.0.1:18092/ping" >"$TMP_DIR/generated_ping.json"
assert_file_contains "$TMP_DIR/generated_ping.json" '"message":"pong"' "generated ping response"

log "users API HTTP smoke"
USERS_API="$TMP_DIR/users_api"
cp -R "$ROOT_DIR/examples/users_api" "$USERS_API"
sed 's/port = 8080/port = 18093/; s/127\.0\.0\.1:8080/127.0.0.1:18093/' \
    "$USERS_API/are.toml" >"$USERS_API/are.toml.tmp"
mv "$USERS_API/are.toml.tmp" "$USERS_API/are.toml"

start_server users "$USERS_API"
users_log="$LAST_LOG_FILE"
wait_for_http "http://127.0.0.1:18093/health" "$users_log"

curl -fsS "http://127.0.0.1:18093/health" >"$TMP_DIR/health.json"
assert_file_contains "$TMP_DIR/health.json" '"status":"ok"' "health response"

invalid_status="$(
    curl -sS -o "$TMP_DIR/invalid_email.json" -w '%{http_code}' \
        -X POST "http://127.0.0.1:18093/users" \
        -H 'content-type: application/json' \
        -d '{"email":"invalid","name":"Ada"}'
)"
assert_eq "$invalid_status" "400" "invalid email HTTP status"
assert_file_contains "$TMP_DIR/invalid_email.json" '"error":"invalid_email"' "invalid email response"

created_status="$(
    curl -sS -o "$TMP_DIR/created_user.json" -w '%{http_code}' \
        -X POST "http://127.0.0.1:18093/users" \
        -H 'content-type: application/json' \
        -d '{"email":"ada@example.com","name":"Ada Lovelace"}'
)"
assert_eq "$created_status" "201" "create user HTTP status"
assert_file_contains "$TMP_DIR/created_user.json" '"id":1' "create user response"
assert_file_contains "$TMP_DIR/created_user.json" '"email":"ada@example.com"' "create user response"

get_status="$(curl -sS -o "$TMP_DIR/get_user.json" -w '%{http_code}' "http://127.0.0.1:18093/users/1")"
assert_eq "$get_status" "200" "get user HTTP status"
assert_file_contains "$TMP_DIR/get_user.json" '"name":"Ada Lovelace"' "get user response"

log "MVP smoke passed"
