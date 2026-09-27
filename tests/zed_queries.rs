#[test]
#[ignore = "set ZED_JUST_EXTENSION to the zed-just checkout"]
fn extension_queries_and_fixture_parse() {
  let root = std::path::PathBuf::from(
    std::env::var_os("ZED_JUST_EXTENSION")
      .expect("ZED_JUST_EXTENSION must point to zed-just"),
  );
  // SAFETY: The linked parser provides a static language definition.
  let language = unsafe { just_lsp::tree_sitter_just() };
  for entry in std::fs::read_dir(root.join("languages/just")).unwrap() {
    let path = entry.unwrap().path();
    if path.extension().is_some_and(|ext| ext == "scm") {
      let source = std::fs::read_to_string(&path).unwrap();
      tree_sitter::Query::new(&language, &source)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    }
  }
  for file in ["tests/modern.justfile", "tests/modern-format.just"] {
    let source = std::fs::read_to_string(root.join(file)).unwrap();
    let document = just_lsp::Document::from(source.as_str());
    assert!(!document.tree.root_node().has_error(), "{file}");
  }
}
