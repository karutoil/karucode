#!/usr/bin/env python3
"""Tests for the repository's public issue intake.

The issue tracker is the only door into this project for someone who has not
cloned it, and two properties have to hold for that door to open. GitHub enforces
neither of them:

  * the repository must offer a blank issue, not only forms, so a report that
    fits no template still has somewhere to go (`blank_issues_enabled`);
  * every template must be something a browser can render. GitHub reads `.yml`
    in `.github/ISSUE_TEMPLATE` as an issue form, but a `.md` file it shows as
    markdown — the body lands in the reporter's issue verbatim. The two forms
    here were once YAML saved as `.md`, so a reporter's issue body was the raw
    YAML: not a permission problem, and not one GitHub would ever tell us about.

Each rule is exercised against a template that violates exactly it, and then the
files in this tree are checked, so the next regression is a red test instead of a
stranger pasting YAML at us.
"""

import pathlib
import re
import tempfile
import unittest

import yaml

ROOT = pathlib.Path(__file__).resolve().parents[1]
TEMPLATES = ROOT / ".github" / "ISSUE_TEMPLATE"

# The field types in GitHub's form schema (docs.github.com, "Syntax for GitHub's
# form schema"). `upload` and `checkboxes` are newer; none of the templates here
# has to use them, but a future one may.
FORM_TYPES = {"markdown", "input", "textarea", "dropdown", "checkboxes", "upload"}
NO_ATTRIBUTES = {"markdown"}
FIELD_ID = re.compile(r"^[A-Za-z0-9_-]+$")

GOOD_FORM = """\
name: Bug report
description: Something does not work the way this repository says it does.
title: "[bug] "
labels: ["bug"]
body:
  - type: markdown
    attributes:
      value: |
        A reproduction beats a description.
  - type: input
    id: version
    attributes:
      label: karucode version
      description: Output of `karucode --version`.
    validations:
      required: true
"""

GOOD_CONFIG = """\
blank_issues_enabled: true
contact_links:
  - name: Security
    url: https://example.com/security
    about: Report vulnerabilities privately.
"""


def load(path: pathlib.Path):
    """Parse one template; return (data, problem) and never raise."""
    try:
        return yaml.safe_load(path.read_text(encoding="utf-8")), None
    except yaml.YAMLError as error:
        return None, f"{path.name}: not valid YAML: {error}"


def check_contact_links(config: dict) -> list[str]:
    problems = []
    links = config.get("contact_links", [])
    if not isinstance(links, list):
        return ["config.yml: contact_links must be a list"]
    for index, link in enumerate(links, start=1):
        if not isinstance(link, dict):
            problems.append(f"config.yml: contact_links[{index}] must be a mapping")
            continue
        for key in ("name", "url", "about"):
            value = link.get(key)
            if not isinstance(value, str) or not value.strip():
                problems.append(
                    f"config.yml: contact_links[{index}] needs a non-empty {key}"
                )
        url = link.get("url")
        if isinstance(url, str) and not url.startswith("https://"):
            problems.append(f"config.yml: contact_links[{index}].url must be https")
    return problems


def check_form(name: str, data) -> list[str]:
    """Every rule a form has to satisfy to be usable by a stranger."""
    if not isinstance(data, dict):
        return [f"{name}: must be a YAML mapping, not {type(data).__name__}"]

    problems = []

    title = data.get("name")
    if not isinstance(title, str) or not title.strip():
        problems.append(f"{name}: needs a name")
    elif len(title.strip()) <= 3:
        # GitHub silently drops a template whose name is 3 characters or fewer.
        problems.append(f"{name}: name must be longer than 3 characters")

    description = data.get("description")
    if not isinstance(description, str) or not description.strip():
        problems.append(f"{name}: needs a description; it is what the chooser shows")

    if "title" in data and not isinstance(data["title"], str):
        problems.append(f"{name}: title must be a string")

    labels = data.get("labels", [])
    if not isinstance(labels, list) or not all(isinstance(x, str) for x in labels):
        problems.append(f"{name}: labels must be a list of strings")

    body = data.get("body")
    if not isinstance(body, list) or not body:
        return problems + [f"{name}: body must be a non-empty list of fields"]

    seen_ids = set()
    fillable = 0
    for index, field in enumerate(body, start=1):
        where = f"{name}: body[{index}]"
        if not isinstance(field, dict):
            problems.append(f"{where} must be a mapping")
            continue

        kind = field.get("type")
        if kind not in FORM_TYPES:
            problems.append(
                f"{where} has type {kind!r}; expected one of "
                + ", ".join(sorted(FORM_TYPES))
            )
            continue

        attributes = field.get("attributes")
        if kind not in NO_ATTRIBUTES:
            fillable += 1
            if not isinstance(attributes, dict):
                problems.append(f"{where} ({kind}) needs attributes")
                continue
            label = attributes.get("label")
            if not isinstance(label, str) or not label.strip():
                problems.append(f"{where} ({kind}) needs attributes.label, the "
                                "prompt the reporter reads")
            field_id = field.get("id")
            if not isinstance(field_id, str) or not FIELD_ID.match(field_id):
                problems.append(f"{where} ({kind}) needs an id of letters, digits, "
                                "'-' or '_'")
            elif field_id in seen_ids:
                # Two fields sharing an id lose the second one's answer.
                problems.append(f"{where} reuses the id {field_id!r}")
            else:
                seen_ids.add(field_id)
            if kind == "dropdown":
                options = attributes.get("options")
                if not isinstance(options, list) or not options:
                    problems.append(f"{where} (dropdown) needs options")

        validations = field.get("validations")
        if validations is not None:
            if not isinstance(validations, dict):
                problems.append(f"{where}: validations must be a mapping")
            else:
                for key, value in validations.items():
                    if key != "required":
                        problems.append(f"{where}: unknown validation {key!r}")
                    elif not isinstance(value, bool):
                        problems.append(f"{where}: validations.required must be true "
                                        "or false")

    if not fillable:
        # A form of nothing but markdown is a template with no fields: the
        # reporter gets prose and an empty textarea anyway.
        problems.append(f"{name}: needs at least one field that is not markdown")

    return problems


def check_markdown_template(path: pathlib.Path) -> list[str]:
    """A `.md` template is legitimate only with real frontmatter."""
    text = path.read_text(encoding="utf-8")
    problems = []
    if not text.startswith("---"):
        problems.append(
            f"{path.name}: a markdown template needs `---` frontmatter; a file "
            "whose body is a form belongs in a .yml file, or GitHub puts the "
            "YAML itself into the reporter's issue"
        )
    if re.search(r"^body:\s*$", text, re.MULTILINE):
        problems.append(
            f"{path.name}: contains a form `body:`, so it is a form saved under "
            "the markdown extension; rename it to .yml"
        )
    return problems


def check_directory(directory: pathlib.Path) -> list[str]:
    """Every problem with a `.github/ISSUE_TEMPLATE` directory, newest first."""
    if not directory.is_dir():
        return [f"{directory}: no issue-template directory"]

    problems = []
    config_path = directory / "config.yml"
    if not config_path.exists():
        problems.append("config.yml: missing; without it no blank issue is offered")
    else:
        config, error = load(config_path)
        if error:
            problems.append(error)
        elif not isinstance(config, dict):
            problems.append("config.yml: must be a YAML mapping")
        else:
            if config.get("blank_issues_enabled") is not True:
                problems.append(
                    "config.yml: blank_issues_enabled must be true, so someone "
                    "who fits neither form can still open an issue"
                )
            problems.extend(check_contact_links(config))

    for path in sorted(directory.iterdir()):
        if not path.is_file() or path.name == "config.yml":
            continue
        if path.suffix in (".yml", ".yaml"):
            data, error = load(path)
            if error:
                problems.append(error)
            else:
                problems.extend(check_form(path.name, data))
        elif path.suffix == ".md":
            problems.extend(check_markdown_template(path))
        else:
            problems.append(
                f"{path.name}: GitHub reads only .yml, .yaml and .md templates"
            )

    return problems


def problems_in(files: dict) -> list[str]:
    """Check a synthetic template directory; return its problems."""
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        for name, content in files.items():
            (root / name).write_text(content, encoding="utf-8")
        return check_directory(root)


class IntakeRules(unittest.TestCase):
    def assert_problem(self, files: dict, needle: str):
        problems = problems_in(files)
        self.assertTrue(problems, f"expected {needle!r}, got no problems")
        self.assertTrue(
            any(needle in problem for problem in problems),
            f"expected a problem containing {needle!r}, got: {problems}",
        )

    def test_clean_directory_passes(self):
        self.assertEqual(
            problems_in({"config.yml": GOOD_CONFIG, "bug_report.yml": GOOD_FORM}), []
        )

    def test_a_form_saved_as_markdown_is_rejected(self):
        # The regression this file exists for.
        self.assert_problem(
            {"config.yml": GOOD_CONFIG, "bug_report.md": GOOD_FORM}, "rename it to .yml"
        )

    def test_blank_issues_off_is_rejected(self):
        self.assert_problem(
            {
                "config.yml": GOOD_CONFIG.replace("true", "false"),
                "bug_report.yml": GOOD_FORM,
            },
            "blank_issues_enabled must be true",
        )

    def test_a_missing_config_is_rejected(self):
        self.assert_problem({"bug_report.yml": GOOD_FORM}, "config.yml: missing")

    def test_contact_link_needs_a_name_and_about(self):
        self.assert_problem(
            {
                "config.yml": GOOD_CONFIG.replace(
                    "  - name: Security\n    url: https://example.com/security\n"
                    "    about: Report vulnerabilities privately.\n",
                    "  - name: Security\n    url: https://example.com/security\n",
                ),
                "bug_report.yml": GOOD_FORM,
            },
            "needs a non-empty about",
        )

    def test_a_short_name_is_rejected(self):
        self.assert_problem(
            {
                "config.yml": GOOD_CONFIG,
                "bug_report.yml": GOOD_FORM.replace("name: Bug report", "name: Bug"),
            },
            "longer than 3 characters",
        )

    def test_a_form_without_a_description_is_rejected(self):
        self.assert_problem(
            {
                "config.yml": GOOD_CONFIG,
                "bug_report.yml": GOOD_FORM.replace(
                    "description: Something does not work the way this repository "
                    "says it does.",
                    "",
                ),
            },
            "needs a description",
        )

    def test_a_form_without_a_body_is_rejected(self):
        self.assert_problem(
            {
                "config.yml": GOOD_CONFIG,
                "bug_report.yml": "name: Bug report\ndescription: A bug.\n",
            },
            "body must be a non-empty list",
        )

    def test_a_form_of_only_markdown_is_rejected(self):
        self.assert_problem(
            {
                "config.yml": GOOD_CONFIG,
                "bug_report.yml": (
                    "name: Bug report\ndescription: A bug.\nbody:\n"
                    "  - type: markdown\n    attributes:\n      value: hi\n"
                ),
            },
            "at least one field that is not markdown",
        )

    def test_an_unknown_field_type_is_rejected(self):
        self.assert_problem(
            {
                "config.yml": GOOD_CONFIG,
                "bug_report.yml": GOOD_FORM.replace("type: input", "type: radios"),
            },
            "has type 'radios'",
        )

    def test_duplicate_field_ids_are_rejected(self):
        duplicated = GOOD_FORM + (
            "  - type: input\n    id: version\n    attributes:\n      label: Again\n"
        )
        self.assert_problem(
            {"config.yml": GOOD_CONFIG, "bug_report.yml": duplicated},
            "reuses the id 'version'",
        )

    def test_a_field_without_a_label_is_rejected(self):
        self.assert_problem(
            {
                "config.yml": GOOD_CONFIG,
                "bug_report.yml": GOOD_FORM.replace(
                    "      label: karucode version\n", ""
                ),
            },
            "needs attributes.label",
        )

    def test_a_dropdown_without_options_is_rejected(self):
        self.assert_problem(
            {
                "config.yml": GOOD_CONFIG,
                "bug_report.yml": GOOD_FORM.replace("type: input", "type: dropdown"),
            },
            "needs options",
        )

    def test_required_must_be_a_boolean(self):
        self.assert_problem(
            {
                "config.yml": GOOD_CONFIG,
                "bug_report.yml": GOOD_FORM.replace("required: true", 'required: "yes"'),
            },
            "validations.required must be true or false",
        )

    def test_invalid_yaml_is_rejected(self):
        self.assert_problem(
            {"config.yml": GOOD_CONFIG, "bug_report.yml": "name: [unclosed\n"},
            "not valid YAML",
        )


class RealTemplates(unittest.TestCase):
    def test_the_actual_template_directory_passes(self):
        problems = check_directory(TEMPLATES)
        self.assertEqual(problems, [], "\n".join(problems))

    def test_the_repository_offers_a_blank_issue(self):
        # Stated separately from the directory check because it is the promise a
        # stranger depends on: somebody whose report fits no form still gets in.
        config = yaml.safe_load((TEMPLATES / "config.yml").read_text(encoding="utf-8"))
        self.assertIs(config.get("blank_issues_enabled"), True)


if __name__ == "__main__":
    unittest.main()