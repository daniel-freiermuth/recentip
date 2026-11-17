Spec baseline (researched 2026-11):

- `specs` submodule tracks upstream `https://github.com/some-ip-com/open-someip-spec.git`
  (no fork). Pinned at `5acd22e590` = Release 26-11_v12 + fixes.
- Check for new upstream releases:
  `curl -s https://api.github.com/repos/some-ip-com/open-someip-spec/compare/<pin>...main`
  (`ahead_by`, commit list).
- Bump: `git -C specs fetch && git -C specs checkout <sha>`, then
  `python3 tools/extract_requirements.py` (rewrites `spec-data/requirements.json` and
  `requirements-summary.md`). Diff old vs new JSON by `id` for added IDs, `reqtype`
  changes (Information → Requirement) and `text` changes; re-check every `covers!` citing a
  changed ID. Update README "Specification Compliance" commit link.
- 2025-12 → 26-11_v12 delta: +`feat_req_someipsd_210` (sorted entries, implemented via
  `SdMessage::sort_entries`), 28 Information → Requirement (27 IPv6 SD options + 752),
  6 text changes. IPv6 is Epic 6 / `docs/MODERNIZATION-ipv6-sd-options.md`.
- Many SD header tests use pre-open-spec IDs (141/142/144) for header fields; real ID is
  `feat_req_someipsd_26`. Verify the spec text before citing an ID.
- The extractor truncates requirement text at 500 chars.
