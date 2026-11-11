English · [简体中文](pull_request_template.zh-CN.md)

## Change

Describe the trigger and resulting behavior. Keep independent fixes in separate commits.

## Validation

List checks actually run and any material limitations.

## Compatibility

Describe migration steps for incompatible changes, or state that none are needed.

## Release note

PR titles and bodies need English only. For user-facing changes, add a bilingual `.changes/<slug>.toml` fragment and reference its path here. See the [commit guide](../docs/development/commits.md). Do not repeat the translations in this PR.

For internal-only changes without a fragment, add the English `release-note:none:` reason marker documented in that guide.
