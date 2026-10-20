#!/usr/bin/env bash
# Tests install.sh against made-up packages, Steam folders and games, the
# way players run it: through its exit code and the files it leaves. CI
# runs it on Linux and, with macOS's own bash 3.2, on macOS; so can anyone:
#
#   bash packaging/unix/test-install.sh
set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd -P)
# Physical paths: macOS keeps temporary files under /var, a link to
# /private/var, and the installer records physical paths.
ROOT=$(cd "$(mktemp -d "${TMPDIR:-/tmp}/tpf3mp-install-test.XXXXXX")" && pwd -P)
BASH_TO_TEST=${BASH_TO_TEST:-bash}
FAILURES=0
OUTPUT=""
case "$(uname -s)" in
  Darwin) DATA_IN_HOME="Library/Application Support/TPF3-MP" ;;
  *) DATA_IN_HOME=".local/share/TPF3-MP" ;;
esac

write() { # PATH TEXT
  mkdir -p "$(dirname "$1")"
  printf '%s' "$2" >"$1"
}

# A package with the mod, a Steam folder with the game in a library and an
# account that has played it, and a home of the test's own for what the
# installer keeps.
setup() { # NAME
  S="$ROOT/$1"
  PACKAGE="$S/package"
  STEAM="$S/Steam"
  local library="$S/Library"
  GAME="$library/steamapps/common/Transport Fever 3"
  MODS="$STEAM/userdata/12345/3493540/local/staging_area"
  DATA="$S/home/$DATA_IN_HOME"
  RECORD="$DATA/installed.txt"
  XDG=""
  mkdir -p "$PACKAGE" "$S/home" "$(dirname "$MODS")"
  cp "$HERE/install.sh" "$HERE/uninstall.sh" "$PACKAGE/"
  write "$PACKAGE/tpf3mp-package.json" '{"version":"9.8.7","platform":"test"}'
  write "$PACKAGE/mod/tpf3mp_1/mod.lua" '-- mod'
  write "$PACKAGE/mod/tpf3mp_1/res/x.lua" '-- x'
  write "$GAME/TransportFever3" 'game'
  write "$STEAM/steamapps/libraryfolders.vdf" "$(printf '"libraryfolders"\n{\n\t"0"\n\t{\n\t\t"path"\t\t"%s"\n\t}\n}\n' "$library")"
  write "$library/steamapps/appmanifest_3493540.acf" "$(printf '"AppState"\n{\n\t"appid"\t\t"3493540"\n\t"installdir"\t\t"Transport Fever 3"\n}\n')"
}

# Runs the package's installer as a player would; sets CODE and OUTPUT.
run() { # SCRIPT ARGS...
  local script=$1
  shift
  OUTPUT=$(cd "$S" && HOME="$S/home" XDG_DATA_HOME="$XDG" "$BASH_TO_TEST" "$PACKAGE/$script" --steam-root "$STEAM" "$@" 2>&1)
  CODE=$?
}
install() { run install.sh "$@"; }

# Ends the test, which runs in a subshell, at the first thing that is not
# as expected. (`set -e` would not: bash ignores it in a tested command.)
check() { # DESCRIPTION CONDITION...
  local what=$1
  shift
  if ! "$@"; then
    printf 'expected: %s\n%s\n' "$what" "$OUTPUT" >&2
    exit 1
  fi
}
same() { [ "$(cat "$1" 2>/dev/null)" = "$2" ]; }
code() { [ "$CODE" = "$1" ]; }
# Nothing in the game's folder but the game.
game_untouched() { [ "$(ls -A "$GAME")" = TransportFever3 ] && same "$GAME/TransportFever3" game; }

t() { # NAME FUNCTION
  if ("$2"); then
    echo "ok   $1"
  else
    echo "FAIL $1"
    FAILURES=$((FAILURES + 1))
  fi
}

installs_and_uninstalls() {
  setup install
  install
  check 'installed' code 0
  check "the mod in Steam's mods folder" same "$MODS/tpf3mp_1/res/x.lua" '-- x'
  check 'the version recorded' grep -qx 'version=9.8.7' "$RECORD"
  check 'the mod recorded' grep -qx "mod=$MODS/tpf3mp_1" "$RECORD"
  check "nothing in the game's folder" game_untouched
  check 'no launch option to set' eval '! grep -q LD_PRELOAD <<<"$OUTPUT"'

  # Again, with a newer mod: the old one goes to the backups.
  write "$PACKAGE/mod/tpf3mp_1/res/x.lua" '-- x 2'
  install
  check 'reinstalled' code 0
  check 'the newer mod' same "$MODS/tpf3mp_1/res/x.lua" '-- x 2'
  check 'no staging folder left' [ "$(ls -A "$MODS")" = tpf3mp_1 ]

  run uninstall.sh
  check 'uninstalled' code 0
  check 'the mod gone' [ ! -e "$MODS/tpf3mp_1" ]
  check 'the record gone' [ ! -e "$RECORD" ]
  check "the game's folder untouched" game_untouched
  check 'the mods kept in the backups' [ "$(find "$DATA/backups" -name x.lua | wc -l)" -ge 2 ]
  run uninstall.sh
  check 'nothing left to uninstall' code 1
}

installs_into_a_folder_given() {
  setup given
  install "$S/elsewhere"
  check 'installed' code 0
  check 'the mod where it was told' same "$S/elsewhere/tpf3mp_1/mod.lua" '-- mod'
  check 'nothing in Steam' [ ! -e "$MODS" ]
}

keeps_its_record_where_the_launcher_reads_it() {
  setup xdg
  XDG="$S/xdg"
  install
  check 'installed' code 0
  check 'the record in XDG_DATA_HOME' grep -qx "mod=$MODS/tpf3mp_1" "$S/xdg/TPF3-MP/installed.txt"
  # Not a full path: the launcher does not use it, so neither does this.
  setup relative
  XDG="relative"
  install
  check 'installed' code 0
  check 'the record in the home' [ -f "$RECORD" ]
}

refuses_a_record_naming_anything_but_the_mod() {
  setup record
  install
  check 'installed' code 0
  local record bad
  record=$(cat "$RECORD")
  for bad in 'mod=/etc' "mod=$MODS/../tpf3mp_1" 'mod=tpf3mp_1' 'rm=-rf /'; do
    printf '%s\n%s\n' "$record" "$bad" >"$RECORD"
    install
    check "refused $bad" code 1
    run uninstall.sh
    check "uninstall refused $bad" code 1
    check 'the mod untouched' same "$MODS/tpf3mp_1/mod.lua" '-- mod'
  done
}

says_when_steam_has_no_mods_folder() {
  setup fresh
  rm -rf "$STEAM/userdata"
  install
  check 'refused' code 1
  check 'says to start the game once' grep -q 'start the game once' <<<"$OUTPUT"
  check 'no record' [ ! -e "$RECORD" ]
}

refuses_while_the_game_runs() {
  setup running
  # Multicall coreutils (Ubuntu 26.04) dispatches by argv[0], so keep the
  # sleep basename. The installer checks the game folder, not its name.
  cp "$(command -v sleep)" "$GAME/sleep"
  "$GAME/sleep" 30 &
  local game=$!
  trap "kill $game 2>/dev/null || true; wait $game 2>/dev/null || true" EXIT
  sleep 0.5
  OUTPUT="fake game pid=$game executable=$GAME/sleep"
  check 'the fake game remains running' kill -0 "$game"
  local command
  command=$(ps -p "$game" -o args=)
  OUTPUT="fake game pid=$game command=$command"
  check 'the fake game is visible under its game folder' test "$command" = "$GAME/sleep 30"
  printf '%s\n' "$OUTPUT"
  install
  kill "$game" 2>/dev/null || true
  wait "$game" 2>/dev/null || true
  trap - EXIT
  check 'refused' code 1
  check 'says to close the game' grep -q 'Close Transport Fever 3' <<<"$OUTPUT"
  check 'no mod' [ ! -e "$MODS/tpf3mp_1" ]
}

puts_everything_back_when_a_step_fails() {
  setup rollback
  install
  check 'installed' code 0
  # The record cannot be rewritten: a folder stands in its way.
  rm "$RECORD"
  mkdir "$RECORD"
  write "$PACKAGE/mod/tpf3mp_1/mod.lua" '-- new'
  install
  check 'failed' code 1
  check 'the installed mod put back' same "$MODS/tpf3mp_1/mod.lua" '-- mod'
  check 'nothing left over' [ "$(ls -A "$MODS")" = tpf3mp_1 ]
  check 'says nothing changed' grep -q 'Nothing was changed' <<<"$OUTPUT"
}

t "installs the mod in Steam's mods folder, and nothing in the game's" installs_and_uninstalls
t 'installs into a mods folder given' installs_into_a_folder_given
case "$(uname -s)" in
  Darwin) ;;
  *) t 'keeps its record where the launcher reads it' keeps_its_record_where_the_launcher_reads_it ;;
esac
t 'refuses a record that names anything but the mod' refuses_a_record_naming_anything_but_the_mod
t 'says when Steam has no mods folder for the game yet' says_when_steam_has_no_mods_folder
case "$(uname -s)" in
  # Git Bash's ps is not the one players' systems have.
  MINGW* | MSYS* | CYGWIN*) echo "skip refuses while the game runs (a Windows shell)" ;;
  *) t 'refuses while the game runs' refuses_while_the_game_runs ;;
esac
t 'puts everything back when a step fails' puts_everything_back_when_a_step_fails

rm -rf "$ROOT"
if [ "$FAILURES" -gt 0 ]; then
  echo "$FAILURES installer test(s) failed"
  exit 1
fi
echo 'all installer tests passed'
