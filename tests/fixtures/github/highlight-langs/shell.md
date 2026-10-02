```shell
#!/usr/bin/env bash
set -euo pipefail

NAME="${1:-world}"
readonly DIR=$(cd "$(dirname "$0")" && pwd)

greet() {
  local who="$1"
  echo "Hello, $who" >&2
}

if [[ -f "$DIR/config" ]]; then
  source "$DIR/config"
elif [ $# -gt 1 ]; then
  exit 1
fi

for f in *.txt; do
  grep -q 'pattern' "$f" && rm -f -- "$f"
done

case "$NAME" in
  a*) greet "$NAME" ;;
  *) export PATH="$HOME/bin:$PATH" ;;
esac
cat <<EOF
heredoc $NAME
EOF
arr=(one two) ; echo "${arr[@]}" | sed -e 's/a/b/g'
```
