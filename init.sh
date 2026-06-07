#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT_DIR"

OLD_DISPLAY="AsterForge"
OLD_FLAT="asterforge"
OLD_SNAKE="aster_forge"
OLD_KEBAB="aster-forge"
OLD_UPPER="ASTERFORGE"
OLD_REPO_URL="https://github.com/AsterCommunity/AsterForge"
OLD_REPO_PATH="AsterCommunity/AsterForge"

PROJECT_NAME="${PROJECT_NAME:-}"
CRATE_NAME="${CRATE_NAME:-}"
FLAT_SLUG="${FLAT_SLUG:-}"
UPPER_TOKEN="${UPPER_TOKEN:-}"
REPOSITORY_URL="${REPOSITORY_URL:-}"
DRY_RUN=0
YES=0

usage() {
    cat <<'EOF'
Usage:
  ./init.sh [options]

Options:
  --name <name>       Display/project name replacing AsterForge, e.g. AsterDrive
  --crate <name>      Rust crate/binary name replacing aster_forge, e.g. aster_drive
  --slug <name>       Lowercase flat slug replacing asterforge, e.g. asterdrive
  --upper <name>      Uppercase token replacing ASTERFORGE, e.g. ASTERDRIVE
  --repo <url>        Repository URL replacing https://github.com/AsterCommunity/AsterForge
  --yes               Do not ask for confirmation
  --dry-run           Show what would be changed without editing files
  -h, --help          Show this help

Environment variables with the same uppercase names are also accepted:
  PROJECT_NAME, CRATE_NAME, FLAT_SLUG, UPPER_TOKEN, REPOSITORY_URL
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --name)
            PROJECT_NAME="${2:-}"
            shift 2
            ;;
        --crate)
            CRATE_NAME="${2:-}"
            shift 2
            ;;
        --slug)
            FLAT_SLUG="${2:-}"
            shift 2
            ;;
        --upper)
            UPPER_TOKEN="${2:-}"
            shift 2
            ;;
        --repo)
            REPOSITORY_URL="${2:-}"
            shift 2
            ;;
        --yes)
            YES=1
            shift
            ;;
        --dry-run)
            DRY_RUN=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "unknown option: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

require_command() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "missing required command: $1" >&2
        exit 1
    fi
}

split_words() {
    perl -CS -Mutf8 -e '
        my $value = shift // "";
        $value =~ s/([a-z0-9])([A-Z])/$1 $2/g;
        $value =~ s/[^A-Za-z0-9]+/ /g;
        $value =~ s/^\s+|\s+$//g;
        print lc($value);
    ' -- "$1"
}

words_to_pascal() {
    awk '{
        for (i = 1; i <= NF; i++) {
            printf toupper(substr($i, 1, 1)) substr($i, 2)
        }
    }'
}

words_to_separator() {
    local separator="$1"
    awk -v sep="$separator" '{
        for (i = 1; i <= NF; i++) {
            if (i > 1) printf sep
            printf $i
        }
    }'
}

prompt_value() {
    local label="$1"
    local default_value="$2"
    local value=""

    if [ -t 0 ]; then
        printf "%s [%s]: " "$label" "$default_value" >&2
        read -r value
    fi

    if [ -n "$value" ]; then
        printf "%s" "$value"
    else
        printf "%s" "$default_value"
    fi
}

derive_repo_path() {
    local url="$1"
    printf "%s" "$url" \
        | sed -E 's#^https?://github.com/##; s#^git@github.com:##; s#\.git$##'
}

validate_inputs() {
    if [ -z "$PROJECT_NAME" ]; then
        echo "project name cannot be empty" >&2
        exit 1
    fi
    case "$PROJECT_NAME" in
        *$'\n'*|*/*)
            echo "project name must not contain newlines or slashes" >&2
            exit 1
            ;;
    esac
    if [[ ! "$CRATE_NAME" =~ ^[a-z][a-z0-9_]*$ ]]; then
        echo "crate name must match ^[a-z][a-z0-9_]*$: $CRATE_NAME" >&2
        exit 1
    fi
    if [[ ! "$FLAT_SLUG" =~ ^[a-z][a-z0-9]*$ ]]; then
        echo "flat slug must match ^[a-z][a-z0-9]*$: $FLAT_SLUG" >&2
        exit 1
    fi
    if [[ ! "$UPPER_TOKEN" =~ ^[A-Z][A-Z0-9_]*$ ]]; then
        echo "upper token must match ^[A-Z][A-Z0-9_]*$: $UPPER_TOKEN" >&2
        exit 1
    fi
    if [ -z "$REPOSITORY_URL" ]; then
        echo "repository URL cannot be empty" >&2
        exit 1
    fi
}

should_skip_file() {
    case "$1" in
        ./target/*|\
        ./tmp/*|\
        ./data/*|\
        ./coverage/*|\
        ./playwright-report/*|\
        ./frontend-panel/node_modules/*|\
        ./frontend-panel/dist/*|\
        ./frontend-panel/dev-dist/*|\
        ./frontend-panel/generated/*|\
        ./frontend-panel/data/*|\
        ./frontend-panel/playwright-report/*|\
        ./frontend-panel/test-results/*|\
        ./frontend-panel/.playwright/*|\
        ./frontend-panel/coverage/*|\
        ./docs/node_modules/*|\
        ./docs/.vitepress/dist/*|\
        ./docs/.vitepress/cache/*)
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}

is_text_file() {
    [ ! -s "$1" ] && return 0
    LC_ALL=C grep -Iq . "$1"
}

contains_old_marker() {
    local file="$1"
    grep -qF "$OLD_DISPLAY" "$file" \
        || grep -qF "$OLD_FLAT" "$file" \
        || grep -qF "$OLD_SNAKE" "$file" \
        || grep -qF "$OLD_KEBAB" "$file" \
        || grep -qF "$OLD_UPPER" "$file" \
        || grep -qF "$OLD_REPO_URL" "$file" \
        || grep -qF "$OLD_REPO_PATH" "$file"
}

replace_literal_in_file() {
    local file="$1"
    local old="$2"
    local new="$3"
    perl -0pi -e 'BEGIN { our ($old, $new) = splice(@ARGV, 0, 2) } s/\Q$old\E/$new/g' "$old" "$new" "$file"
}

apply_replacements() {
    local file="$1"
    local repo_path="$2"
    local kebab_slug="$3"

    replace_literal_in_file "$file" "$OLD_REPO_URL" "$REPOSITORY_URL"
    replace_literal_in_file "$file" "$OLD_REPO_PATH" "$repo_path"
    replace_literal_in_file "$file" "$OLD_UPPER" "$UPPER_TOKEN"
    replace_literal_in_file "$file" "$OLD_SNAKE" "$CRATE_NAME"
    replace_literal_in_file "$file" "$OLD_KEBAB" "$kebab_slug"
    replace_literal_in_file "$file" "$OLD_FLAT" "$FLAT_SLUG"
    replace_literal_in_file "$file" "$OLD_DISPLAY" "$PROJECT_NAME"
}

collect_files() {
    if command -v git >/dev/null 2>&1 && git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
        git ls-files -co --exclude-standard -z
    else
        find . \
            -path './.git' -prune -o \
            -path './target' -prune -o \
            -path './tmp' -prune -o \
            -path './data' -prune -o \
            -path './coverage' -prune -o \
            -path './playwright-report' -prune -o \
            -path './frontend-panel/node_modules' -prune -o \
            -path './frontend-panel/dist' -prune -o \
            -path './frontend-panel/dev-dist' -prune -o \
            -path './frontend-panel/generated' -prune -o \
            -path './frontend-panel/data' -prune -o \
            -path './frontend-panel/playwright-report' -prune -o \
            -path './frontend-panel/test-results' -prune -o \
            -path './frontend-panel/.playwright' -prune -o \
            -path './frontend-panel/coverage' -prune -o \
            -path './docs/node_modules' -prune -o \
            -path './docs/.vitepress/dist' -prune -o \
            -path './docs/.vitepress/cache' -prune -o \
            -type f -print0
    fi
}

require_command perl
require_command awk
require_command sed
require_command grep

default_words="$(split_words "$(basename "$ROOT_DIR")")"
if [ -z "$default_words" ]; then
    default_words="my service"
fi

default_display="$(printf "%s\n" "$default_words" | words_to_pascal)"
default_snake="$(printf "%s\n" "$default_words" | words_to_separator "_")"
default_flat="$(printf "%s\n" "$default_words" | words_to_separator "")"
default_upper="$(printf "%s" "$default_flat" | tr '[:lower:]' '[:upper:]')"
default_repo="https://github.com/AsterCommunity/${default_display}"

if [ -z "$PROJECT_NAME" ]; then
    PROJECT_NAME="$(prompt_value "Project display name" "$default_display")"
fi
project_words="$(split_words "$PROJECT_NAME")"
if [ -z "$project_words" ]; then
    project_words="$default_words"
fi

if [ -z "$CRATE_NAME" ]; then
    CRATE_NAME="$(prompt_value "Rust crate/binary name" "$(printf "%s\n" "$project_words" | words_to_separator "_")")"
fi
if [ -z "$FLAT_SLUG" ]; then
    FLAT_SLUG="$(prompt_value "Lowercase flat slug" "$(printf "%s\n" "$project_words" | words_to_separator "")")"
fi
if [ -z "$UPPER_TOKEN" ]; then
    UPPER_TOKEN="$(prompt_value "Uppercase frontend token" "$(printf "%s" "$FLAT_SLUG" | tr '[:lower:]' '[:upper:]')")"
fi
if [ -z "$REPOSITORY_URL" ]; then
    REPOSITORY_URL="$(prompt_value "Repository URL" "$default_repo")"
fi

validate_inputs

kebab_slug="$(printf "%s\n" "$project_words" | words_to_separator "-")"
repo_path="$(derive_repo_path "$REPOSITORY_URL")"

echo "AsterForge template initialization"
echo
echo "  $OLD_DISPLAY -> $PROJECT_NAME"
echo "  $OLD_FLAT -> $FLAT_SLUG"
echo "  $OLD_SNAKE -> $CRATE_NAME"
echo "  $OLD_KEBAB -> $kebab_slug"
echo "  $OLD_UPPER -> $UPPER_TOKEN"
echo "  $OLD_REPO_URL -> $REPOSITORY_URL"
echo "  $OLD_REPO_PATH -> $repo_path"
echo

if [ "$YES" -ne 1 ]; then
    printf "Apply these replacements? [y/N]: " >&2
    read -r answer
    case "$answer" in
        y|Y|yes|YES)
            ;;
        *)
            echo "aborted"
            exit 0
            ;;
    esac
fi

changed_files=0
while IFS= read -r -d '' file; do
    case "$file" in
        ./*)
            ;;
        *)
            file="./$file"
            ;;
    esac

    if should_skip_file "$file"; then
        continue
    fi
    if ! is_text_file "$file"; then
        continue
    fi
    if ! contains_old_marker "$file"; then
        continue
    fi

    changed_files=$((changed_files + 1))
    if [ "$DRY_RUN" -eq 1 ]; then
        printf "would update %s\n" "${file#./}"
    else
        apply_replacements "$file" "$repo_path" "$kebab_slug"
        printf "updated %s\n" "${file#./}"
    fi
done < <(collect_files)

echo
if [ "$DRY_RUN" -eq 1 ]; then
    echo "dry run complete: $changed_files file(s) would be updated"
else
    echo "initialization complete: $changed_files file(s) updated"
    echo "next steps:"
    echo "  cargo fmt"
    echo "  cargo generate-lockfile"
    echo "  cargo check --bins"
    echo "  cd frontend-panel && bun install && bun run check"
fi
