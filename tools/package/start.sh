#!/bin/sh
set -eu
directory=$(CDPATH= cd -P -- "$(dirname -- "$0")" && pwd -P)
exec "$directory/fixtester" "$@"
