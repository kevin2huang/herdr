#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd -- "$script_dir/../../../.." && pwd)
command_name=${1:-doctor}

zig_bin=${ZIG:-}
if [[ -z "$zig_bin" ]]; then
  zig_bin=$(command -v zig || true)
fi

doctor() {
  [[ -f "$repo_root/Cargo.toml" ]] || { echo "error: Herdr repository root not found" >&2; return 1; }
  command -v cargo >/dev/null || { echo "error: cargo is not on PATH" >&2; return 1; }
  [[ -n "$zig_bin" && -x "$zig_bin" ]] || { echo "error: set ZIG to a Zig 0.16.0 executable" >&2; return 1; }
  local zig_version
  zig_version=$($zig_bin version)
  [[ "$zig_version" == "0.16.0" ]] || { echo "error: Herdr requires Zig 0.16.0; found $zig_version at $zig_bin" >&2; return 1; }
  echo "repo: $repo_root"
  echo "cargo: $(cargo --version)"
  echo "zig: $zig_version ($zig_bin)"
}

case "$command_name" in
  doctor)
    doctor
    ;;
  pane-backgrounds)
    doctor
    evidence_dir=${2:-/private/tmp/herdr-pane-background-proof-$(date +%Y%m%d-%H%M%S)-$$}
    mkdir -p "$evidence_dir"
    printf '%s\n' \
      "feature: pane-backgrounds" \
      "repo: $repo_root" \
      "zig: $zig_bin" \
      >"$evidence_dir/action.txt"
    cd "$repo_root"
    ZIG="$zig_bin" cargo test --locked --bin herdr \
      client::shell::tests::chrome_context::pane_default_background_recolors_only_reset_cells_inside_panes \
      -- --exact --nocapture 2>&1 | tee "$evidence_dir/unit.log"
    ZIG="$zig_bin" cargo test --locked --bin herdr \
      client::shell::tests::chrome_context::pane_gap_background_preserves_pane_frames_and_colors_only_unowned_cells \
      -- --exact --nocapture 2>&1 | tee -a "$evidence_dir/unit.log"
    ZIG="$zig_bin" cargo test --locked --bin herdr \
      client::shell::tests::chrome_context::pane_background_includes_visible_and_hidden_scrollbar_lanes \
      -- --exact --nocapture 2>&1 | tee -a "$evidence_dir/unit.log"
    ZIG="$zig_bin" cargo test --locked --bin herdr \
      client::shell::tests::popup_focus_projection::retained_surface_patch_updates_ \
      -- --nocapture 2>&1 | tee -a "$evidence_dir/unit.log"
    ZIG="$zig_bin" cargo test --locked --bin herdr \
      client::shell::tests::popup_focus_projection::client_composes_popup_terminal_content_inside_client_owned_chrome \
      -- --exact --nocapture 2>&1 | tee -a "$evidence_dir/unit.log"
    ZIG="$zig_bin" cargo test --locked --bin herdr \
      client::shell::tests::copy::client_selection_uses_the_configured_pane_default_background \
      -- --exact --nocapture 2>&1 | tee -a "$evidence_dir/unit.log"
    ZIG="$zig_bin" cargo test --locked --bin herdr \
      client::shell::pane_frames::tests:: \
      -- --nocapture 2>&1 | tee -a "$evidence_dir/unit.log"
    HERDR_VISUAL_EVIDENCE_DIR="$evidence_dir" ZIG="$zig_bin" cargo test --locked \
      --test client_mode pane_gap_background_visual_ownership \
      -- --exact --nocapture --test-threads=1 2>&1 | tee "$evidence_dir/visual.log"
    HERDR_VISUAL_EVIDENCE_DIR="$evidence_dir" ZIG="$zig_bin" cargo test --locked \
      --test client_mode pane_sidebar_gap_matches_split_gutter \
      -- --exact --nocapture --test-threads=1 2>&1 | tee -a "$evidence_dir/visual.log"
    HERDR_VISUAL_EVIDENCE_DIR="$evidence_dir" ZIG="$zig_bin" cargo test --locked \
      --test client_mode single_pane_keeps_rounded_focused_frame \
      -- --exact --nocapture --test-threads=1 2>&1 | tee -a "$evidence_dir/visual.log"
    for artifact in raw.ansi streaming.ansi screen.txt config.toml census.txt unit.log visual.log sidebar/raw.ansi sidebar/census.txt single-pane/raw.ansi; do
      [[ -f "$evidence_dir/$artifact" ]] || { echo "error: missing evidence $artifact" >&2; exit 1; }
    done
    echo "pane background verification passed"
    echo "evidence: $evidence_dir"
    ;;
  *)
    echo "usage: verify-herdr.sh doctor | pane-backgrounds [evidence-directory]" >&2
    exit 2
    ;;
esac
