use super::*;

#[derive(Debug)]
pub struct ProjectView<'a> {
  pub(super) document: &'a Document,
  pub(super) documents: Vec<ProjectViewDocument<'a>>,
  pub(super) project: Option<&'a Project>,
  pub(super) store: Option<&'a DocumentStore>,
}

impl<'a> ProjectView<'a> {
  fn declarations<T>(
    &self,
    declarations: impl Fn(&Document) -> Vec<T>,
    declaration_name: impl Fn(&T) -> &str,
    declaration_position: impl Fn(&T) -> lsp::Position,
  ) -> HashMap<String, Located<T>> {
    let mut candidates = Vec::new();

    for (traversal_order, document) in self.documents.iter().enumerate() {
      for declaration in declarations(document.document) {
        candidates.push((traversal_order, document, declaration));
      }
    }

    candidates.sort_by_key(|(traversal_order, document, declaration)| {
      (
        Reverse(document.load_depth),
        *traversal_order,
        declaration_position(declaration),
      )
    });

    candidates
      .into_iter()
      .map(|(_, document, value)| {
        (
          declaration_name(&value).to_owned(),
          Located::new(document.document.uri.clone(), value),
        )
      })
      .collect()
  }

  #[must_use]
  pub fn document(&self) -> &'a Document {
    self.document
  }

  pub(super) fn documents(&self) -> impl Iterator<Item = &'a Document> + '_ {
    self.documents.iter().map(|document| document.document)
  }

  #[must_use]
  pub fn find_function(&self, name: &str) -> Option<Located<Function>> {
    self.resolved_functions().remove(name)
  }

  #[must_use]
  pub fn find_recipe(&self, name: &str) -> Option<Located<Recipe>> {
    if let Some((module, rest)) = name.split_once("::") {
      let project = self.project?;
      let store = self.store?;
      let declaration = self
        .declarations(
          Document::modules,
          |module| &module.name.value,
          |module| module.range.start,
        )
        .remove(module)?;
      let uri = project.modules.get(&declaration.uri)?.get(module)?;
      let scope = ImportScope::for_root(project, uri);
      return Self::new(store.get(uri)?, &scope, store)
        .with_project(project)
        .find_recipe(rest);
    }
    self.resolved_recipes().remove(name)
  }

  #[must_use]
  pub fn find_variable(&self, name: &str) -> Option<Located<Variable>> {
    self
      .declarations(
        Document::variables,
        |variable| &variable.name.value,
        |variable| variable.range.start,
      )
      .remove(name)
  }

  #[must_use]
  pub fn new(
    document: &'a Document,
    import_scope: &ImportScope,
    documents: &'a DocumentStore,
  ) -> Self {
    let documents_store = documents;
    let documents = import_scope
      .documents()
      .iter()
      .filter_map(|scope_document| {
        let scoped_document = if scope_document.uri == document.uri {
          document
        } else {
          documents.get(&scope_document.uri)?
        };

        Some(ProjectViewDocument {
          document: scoped_document,
          load_depth: scope_document.load_depth,
        })
      })
      .collect();

    Self {
      document,
      documents,
      project: None,
      store: Some(documents_store),
    }
  }

  pub(super) fn resolved_functions(
    &self,
  ) -> HashMap<String, Located<Function>> {
    self.declarations(
      Document::functions,
      |function| &function.name.value,
      |function| function.range.start,
    )
  }

  pub(super) fn resolved_recipes(&self) -> HashMap<String, Located<Recipe>> {
    self.declarations(
      Document::recipes,
      |recipe| &recipe.name.value,
      |recipe| recipe.range.start,
    )
  }

  pub(super) fn with_project(mut self, project: &'a Project) -> Self {
    self.project = Some(project);
    self
  }
}

impl<'a> From<&'a Document> for ProjectView<'a> {
  fn from(document: &'a Document) -> Self {
    Self {
      document,
      project: None,
      store: None,
      documents: vec![ProjectViewDocument {
        document,
        load_depth: 0,
      }],
    }
  }
}

#[cfg(test)]
mod tests {
  use {super::*, indoc::indoc, pretty_assertions::assert_eq};

  #[test]
  fn qualified_dependencies_resolve_without_leaking_module_scope() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
      dir.path().join("justfile"),
      "[private]\nmod linting 'lint.just'\ncheck: (linting::all 'true')\n",
    )
    .unwrap();
    fs::write(
      dir.path().join("lint.just"),
      indoc! {
        "
      import 'shared.just'
      mod nested 'nested.just'
      [arg('check', long, value='true')]
      all check='false':
      "
      },
    )
    .unwrap();
    fs::write(dir.path().join("shared.just"), "imported:\n").unwrap();
    fs::write(dir.path().join("nested.just"), "leaf:\n").unwrap();
    let uri = lsp::Url::from_file_path(dir.path().join("justfile")).unwrap();
    let mut store = DocumentStore::default();
    let project = ProjectLoader::load(&mut store, &uri).unwrap();
    let document = store.get(&uri).unwrap();
    let view = ProjectView::new(document, &project.import_scope, &store)
      .with_project(&project);
    for name in ["linting::all", "linting::imported", "linting::nested::leaf"] {
      assert!(view.find_recipe(name).is_some(), "{name}");
    }
    for name in ["all", "imported", "linting::missing", "missing::all"] {
      assert!(view.find_recipe(name).is_none(), "{name}");
    }
    assert!(!document.tree.root_node().has_error());
    assert!(Analyzer { config: None, view }.analyze().is_empty());
  }

  #[test]
  fn qualified_dependencies_use_open_module_contents() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
      dir.path().join("justfile"),
      "mod child\ncheck: child::unsaved\n",
    )
    .unwrap();
    fs::write(dir.path().join("child.just"), "saved:\n").unwrap();
    let uri = lsp::Url::from_file_path(dir.path().join("justfile")).unwrap();
    let mut store = DocumentStore::default();
    store
      .open(lsp::DidOpenTextDocumentParams {
        text_document: lsp::TextDocumentItem {
          uri: lsp::Url::from_file_path(dir.path().join("child.just")).unwrap(),
          language_id: "just".into(),
          version: 1,
          text: "unsaved:\n".into(),
        },
      })
      .unwrap();
    let project = ProjectLoader::load(&mut store, &uri).unwrap();
    let view =
      ProjectView::new(store.get(&uri).unwrap(), &project.import_scope, &store)
        .with_project(&project);
    assert!(view.find_recipe("child::unsaved").is_some());
    assert!(view.find_recipe("child::saved").is_none());
  }

  #[test]
  fn direct_import_overrides_nested_import() {
    let root =
      Document::new("", lsp::Url::parse("file:///justfile").unwrap()).unwrap();

    let direct = Document::new(
      indoc! {
        "
        foo := 'foo'
        foo() := 'foo'
        foo:
          echo foo
        "
      },
      lsp::Url::parse("file:///foo.just").unwrap(),
    )
    .unwrap();

    let nested = Document::new(
      indoc! {
        "
        foo := 'bar'
        foo() := 'bar'
        foo:
          echo bar
        "
      },
      lsp::Url::parse("file:///bar.just").unwrap(),
    )
    .unwrap();

    let view = ProjectView {
      project: None,
      store: None,
      document: &root,
      documents: vec![
        ProjectViewDocument {
          document: &root,
          load_depth: 0,
        },
        ProjectViewDocument {
          document: &direct,
          load_depth: 1,
        },
        ProjectViewDocument {
          document: &nested,
          load_depth: 2,
        },
      ],
    };

    assert_eq!(view.find_recipe("foo").unwrap().uri, direct.uri);
    assert_eq!(view.find_variable("foo").unwrap().uri, direct.uri);
    assert_eq!(view.find_function("foo").unwrap().uri, direct.uri);
  }

  #[test]
  fn equal_depth_imports_use_lifo_precedence() {
    let root =
      Document::new("", lsp::Url::parse("file:///justfile").unwrap()).unwrap();

    let first = Document::new(
      indoc! {
        "
        foo := 'foo'
        foo() := 'foo'
        foo:
          echo foo
        "
      },
      lsp::Url::parse("file:///foo.just").unwrap(),
    )
    .unwrap();

    let second = Document::new(
      indoc! {
        "
        foo := 'bar'
        foo() := 'bar'
        foo:
          echo bar
        "
      },
      lsp::Url::parse("file:///bar.just").unwrap(),
    )
    .unwrap();

    let view = ProjectView {
      project: None,
      store: None,
      document: &root,
      documents: vec![
        ProjectViewDocument {
          document: &root,
          load_depth: 0,
        },
        ProjectViewDocument {
          document: &second,
          load_depth: 1,
        },
        ProjectViewDocument {
          document: &first,
          load_depth: 1,
        },
      ],
    };

    assert_eq!(view.find_recipe("foo").unwrap().uri, first.uri);
    assert_eq!(view.find_variable("foo").unwrap().uri, first.uri);
    assert_eq!(view.find_function("foo").unwrap().uri, first.uri);
  }

  #[test]
  fn later_declaration_in_document_wins() {
    let root = Document::new(
      indoc! {
        "
        foo := 'foo'
        foo() := 'foo'
        foo:
          echo foo

        foo := 'bar'
        foo() := 'bar'
        foo:
          echo bar
        "
      },
      lsp::Url::parse("file:///justfile").unwrap(),
    )
    .unwrap();

    let view = ProjectView::from(&root);

    assert_eq!(view.find_recipe("foo").unwrap().content, "foo:\n  echo bar",);
    assert_eq!(view.find_variable("foo").unwrap().content, "foo := 'bar'",);
    assert_eq!(view.find_function("foo").unwrap().content, "foo() := 'bar'",);
  }

  #[test]
  fn root_overrides_imported_declarations() {
    let root = Document::new(
      indoc! {
        "
        foo := 'foo'
        foo() := 'foo'
        foo:
          echo foo
        "
      },
      lsp::Url::parse("file:///justfile").unwrap(),
    )
    .unwrap();

    let imported = Document::new(
      indoc! {
        "
        foo := 'bar'
        foo() := 'bar'
        foo:
          echo bar
        "
      },
      lsp::Url::parse("file:///foo.just").unwrap(),
    )
    .unwrap();

    let view = ProjectView {
      project: None,
      store: None,
      document: &root,
      documents: vec![
        ProjectViewDocument {
          document: &root,
          load_depth: 0,
        },
        ProjectViewDocument {
          document: &imported,
          load_depth: 1,
        },
      ],
    };

    assert_eq!(view.find_recipe("foo").unwrap().uri, root.uri);
    assert_eq!(view.find_variable("foo").unwrap().uri, root.uri);
    assert_eq!(view.find_function("foo").unwrap().uri, root.uri);
  }
}
