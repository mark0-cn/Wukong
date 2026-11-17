use anyhow::Result;

#[test]
fn generated_catalog_matches_the_executable_registry() {
    assert_eq!(
        wukong::tools::builtin_catalog_markdown(),
        include_str!("../docs/reference/tool-catalog.md")
    );
}

#[test]
fn chinese_catalogs_match_sources_and_preserve_registry_visibility() -> Result<()> {
    use wukong::i18n::Locale;
    let chinese = wukong::tools::builtin_catalog_markdown_for_locale(Locale::ZhCn)?;
    assert_eq!(
        chinese,
        include_str!("../docs/reference/tool-catalog.zh-CN.md")
    );
    assert_eq!(
        wukong::agents::skill_coverage_markdown_for_locale(Locale::ZhCn)?,
        include_str!("../docs/reference/skill-coverage.zh-CN.md")
    );
    for tool in wukong::tools::builtin_tool_definitions() {
        assert!(chinese.contains(&format!("| `{}` |", tool.name)));
    }
    Ok(())
}

#[test]
fn catalog_locale_changes_only_markdown_and_json_stays_identical() -> Result<()> {
    use wukong::i18n::Locale;
    assert_ne!(
        wukong::tools::builtin_catalog_markdown(),
        wukong::tools::builtin_catalog_markdown_for_locale(Locale::ZhCn)?
    );
    assert_eq!(
        wukong::agents::skill_coverage_markdown()?,
        wukong::agents::skill_coverage_markdown_for_locale(Locale::EnUs)?
    );
    assert_ne!(
        wukong::agents::skill_coverage_markdown()?,
        wukong::agents::skill_coverage_markdown_for_locale(Locale::ZhCn)?
    );
    Ok(())
}

#[test]
fn structured_catalog_preserves_the_model_visible_boundary() {
    let definitions = wukong::tools::builtin_tool_definitions();
    let names = definitions
        .iter()
        .map(|definition| definition.name.as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"finish_scan"));
    assert!(names.contains(&"agent_finish"));
    assert!(!names.contains(&"create_agent"));
}

#[test]
fn generated_skill_coverage_matches_registered_associations() -> Result<()> {
    assert_eq!(
        wukong::agents::skill_coverage_markdown()?,
        include_str!("../docs/reference/skill-coverage.md")
    );
    Ok(())
}

#[test]
fn every_current_tool_has_a_display_label_and_unknown_tools_keep_their_name() {
    for tool in wukong::tools::builtin_tool_definitions() {
        let title = wukong::tools::tool_display_title(tool.name.as_str()).expect("missing label");
        assert!(!title.is_empty());
        assert_ne!(title, tool.name.as_str());
        for locale in [wukong::i18n::Locale::EnUs, wukong::i18n::Locale::ZhCn] {
            assert_ne!(locale.text(title), title, "missing localized tool label");
        }
    }
    assert!(wukong::tools::tool_display_title("mcp_custom_tool").is_none());
}
