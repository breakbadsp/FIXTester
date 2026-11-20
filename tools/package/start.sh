#!/bin/sh
set -eu

directory=$(CDPATH= cd -P -- "$(dirname -- "$0")" && pwd -P)
binary="$directory/fixtester"
if [ ! -x "$binary" ]; then
    printf 'Cannot run %s\n' "$binary" >&2
    exit 1
fi

umask 077
mkdir -p "$directory/logs"
stamp=$(date -u +%Y%m%dT%H%M%SZ)
prefix="$directory/logs/fixtester-$stamp"
nohup sh -c '
    prefix=$1
    binary=$2
    shift 2
    exec >"$prefix-$$.log" 2>&1
    printf "FIXTester PID %s\n" "$$"
    exec "$binary" "$@"
' start "$prefix" "$binary" "$@" </dev/null >/dev/null &
pid=$!
printf 'FIXTester PID %s\nLog: %s-%s.log\n' "$pid" "$prefix" "$pid"
