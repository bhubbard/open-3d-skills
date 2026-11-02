#!/usr/bin/env bash
# Install Open 3D Skills for every coding agent on this machine, without Node.
# (With Node 22.20+, `npx skills add bhubbard/open-3d-skills` does the same.)
#
#   curl -fsSL https://raw.githubusercontent.com/bhubbard/open-3d-skills/main/install.sh | bash
#
# Run from the internet, it clones the repo to ~/.open-3d-skills (or updates it) and links the
# skills from there, so `bash ~/.open-3d-skills/install.sh` again (or a git pull) updates them.
# Run from a clone, it links from that clone.
#
#   ./install.sh                     every agent found: Claude Code, Codex, Cursor, Gemini CLI, OpenCode
#   ./install.sh --claude --codex    only these (also --cursor --gemini --opencode --agents)
#   ./install.sh --dir PATH          into any skills folder
#   ./install.sh --copy              copy instead of symlink (Windows, synced folders)
#   ./install.sh threejs-scene       only these skills, by name
#   ./install.sh --list              show the skills
#   ./install.sh --uninstall         remove links and copies this script made
#
# Needs bash and git. Nothing else is installed; the skills' own scripts list what they use.

set -euo pipefail

REPO_URL="https://github.com/bhubbard/open-3d-skills.git"
HOME_CLONE="${OPEN3D_HOME:-$HOME/.open-3d-skills}"
MARK=".open-3d-skills"

SELF="${BASH_SOURCE[0]:-}"
REPO=""
if [[ -n $SELF && -f $SELF ]]; then REPO="$(cd "$(dirname "$SELF")" && pwd)"; fi

# piped from curl (or run outside a clone): fetch the repo, then run the copy inside it
if [[ -z $REPO || ! -d $REPO/skills ]]; then
  command -v git >/dev/null || { echo "git is required" >&2; exit 1; }
  if [[ -d $HOME_CLONE/.git ]]; then
    echo "updating $HOME_CLONE"
    git -C "$HOME_CLONE" pull --ff-only --quiet
  else
    echo "cloning to $HOME_CLONE"
    git clone --depth 1 --quiet "$REPO_URL" "$HOME_CLONE"
  fi
  exec bash "$HOME_CLONE/install.sh" "$@"
fi

SKILLS_DIR="$REPO/skills"
targets=()
mode=install
copy=0
wanted=()

add_target() { targets+=("$1"); }
while [[ $# -gt 0 ]]; do
  case "$1" in
    --claude)   add_target "$HOME/.claude/skills" ;;
    --codex)    add_target "${CODEX_HOME:-$HOME/.codex}/skills" ;;
    --cursor)   add_target "$HOME/.cursor/skills" ;;
    --gemini)   add_target "$HOME/.gemini/skills" ;;
    --opencode) add_target "$HOME/.config/opencode/skills" ;;
    --agents)   add_target "$HOME/.agents/skills" ;;
    --dir)      shift; add_target "${1:?--dir needs a path}" ;;
    --copy)     copy=1 ;;
    --list)     mode=list ;;
    --uninstall) mode=uninstall ;;
    -h|--help)  awk 'NR>1 { if ($0 !~ /^#/) exit; sub(/^# ?/, ""); print }' "$REPO/install.sh"; exit 0 ;;
    -*)         echo "unknown flag: $1" >&2; exit 2 ;;
    *)          wanted+=("$1") ;;
  esac
  shift
done

# no agent named: every agent whose home folder exists
if [[ ${#targets[@]} -eq 0 ]]; then
  [[ -d $HOME/.claude ]] && add_target "$HOME/.claude/skills"
  [[ -d ${CODEX_HOME:-$HOME/.codex} ]] && add_target "${CODEX_HOME:-$HOME/.codex}/skills"
  [[ -d $HOME/.cursor ]] && add_target "$HOME/.cursor/skills"
  [[ -d $HOME/.gemini ]] && add_target "$HOME/.gemini/skills"
  # OpenCode also reads ~/.claude/skills; linking both would list every skill twice
  [[ -d $HOME/.config/opencode && ! -d $HOME/.claude ]] && add_target "$HOME/.config/opencode/skills"
  if [[ ${#targets[@]} -eq 0 ]]; then
    echo "no coding agent found; installing to ~/.claude/skills (use --dir for another place)"
    add_target "$HOME/.claude/skills"
  fi
fi

# frontmatter name: of a SKILL.md, falling back to the folder name
skill_name() {
  awk 'NR==1 && $0!="---" { exit } NR>1 && /^---[[:space:]]*$/ { exit }
       NR>1 && /^name:/ { sub(/^name:[[:space:]]*/, ""); gsub(/^["'\'']|["'\'']$/, ""); print; exit }' "$1"
}
wants() {
  [[ ${#wanted[@]} -eq 0 ]] && return 0
  local w; for w in "${wanted[@]}"; do [[ $w == "$1" ]] && return 0; done
  return 1
}
# true if PATH is a link into this repo, or a copy this script made
ours() {
  local p="$1"
  if [[ -L $p ]]; then [[ "$(readlink "$p")" == "$SKILLS_DIR"/* ]]; return; fi
  [[ -f $p/$MARK ]]
}

if [[ $mode == uninstall ]]; then
  for t in "${targets[@]}"; do
    [[ -d $t ]] || continue
    for d in "$SKILLS_DIR"/*/; do
      n="$(skill_name "$d/SKILL.md")"; n="${n:-$(basename "$d")}"
      p="$t/$n"
      if [[ -e $p || -L $p ]] && ours "$p"; then rm -rf "$p"; echo "removed  $p"; fi
    done
  done
  exit 0
fi

names=()
for d in "$SKILLS_DIR"/*/; do
  d="${d%/}"
  [[ -f $d/SKILL.md ]] || continue
  n="$(skill_name "$d/SKILL.md")"; n="${n:-$(basename "$d")}"
  names+=("$n")
  wants "$n" || continue
  if [[ $mode == list ]]; then printf '%-16s %s\n' "$n" "skills/$(basename "$d")"; continue; fi
  for t in "${targets[@]}"; do
    mkdir -p "$t"
    p="$t/$n"
    if [[ -e $p || -L $p ]]; then
      if ours "$p"; then rm -rf "$p"
      else echo "skip     $p (something else is installed there; move it aside first)" >&2; continue; fi
    fi
    if [[ $copy == 1 ]]; then cp -R "$d" "$p"; touch "$p/$MARK"; echo "copied   $p"
    else ln -s "$d" "$p"; echo "linked   $p"; fi
  done
done

if [[ ${#wanted[@]} -gt 0 ]]; then
  for w in "${wanted[@]}"; do
    found=0; for n in "${names[@]}"; do [[ $n == "$w" ]] && found=1; done
    [[ $found == 1 ]] || { echo "no skill named '$w'; available: ${names[*]}" >&2; exit 1; }
  done
fi
[[ $mode == list ]] || echo "done. Start a new agent session to load the skills."
