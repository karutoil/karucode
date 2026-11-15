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
# form schema"), with the attribute and validation keys each one accepts. A key
# outside its type's set is silently ignored by GitHub, so a misspelled
# `required: true` under the wrong type reads as an optional field forever.
FORM_TYPES = {
    "markdown": {
        "attributes": {"value"},
        "validations": set(),
        "needs_id": False,
    },
    "input": {
        "attributes": {"label", "description", "placeholder", "value"},
        "validations": {"required", "min_length"},
        "needs_id": True,
    },
    "textarea": {
        "attributes": {"label", "description", "placeholder", "value", "render"},
        "validations": {"required", "min_length"},
        "needs_id": True,
    },
    "dropdown": {
        "attributes": {"label", "description", "multiple", "options", "default"},
        "validations": {"required"},
        "needs_id": True,
    },
    "checkboxes": {
        "attributes": {"label", "description", "options"},
        "validations": {"required"},
        "needs_id": True,
    },
    "upload": {
        "attributes": {"label", "description", "accept"},
        "validations": {"required"},
        "needs_id": True,
    },
}
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


def options_of(where: str, kind: str, attributes: dict) -> list[str]:
    """The `options` of a dropdown or a checkbox set, which GitHub requires to be
    non-empty and, for a dropdown, distinct — two identical choices render as one
    and the reporter cannot tell which they picked."""
    problems = []
    options = attributes.get("options")
    if not isinstance(options, list) or not options:
        return [f"{where} ({kind}) needs a non-empty attributes.options"]

    if kind == "dropdown":
        if not all(isinstance(option, str) and option.strip() for option in options):
            problems.append(f"{where} (dropdown): every option must be text")
        elif len(set(options)) != len(options):
            duplicated = sorted({o for o in options if options.count(o) > 1})
            problems.append(
                f"{where} (dropdown): repeated option(s): " + ", ".join(duplicated)
            )
        default = attributes.get("default")
        if default is not None and (
            isinstance(default, bool)
            or not isinstance(default, int)
            or not 0 <= default < len(options)
        ):
            problems.append(
                f"{where} (dropdown): attributes.default must be an index into "
                f"options (0..{len(options) - 1})"
            )
    else:
        for index, option in enumerate(options, start=1):
            if not isinstance(option, dict):
                problems.append(
                    f"{where} (checkboxes): options[{index}] must be a mapping with "
                    "a label"
                )
                continue
            label = option.get("label")
            if not isinstance(label, str) or not label.strip():
                problems.append(
                    f"{where} (checkboxes): options[{index}] needs a label, the text "
                    "beside the box"
                )
            unknown = set(option) - {"label", "required"}
            if unknown:
                problems.append(
                    f"{where} (checkboxes): options[{index}] has key(s) GitHub does "
                    "not accept: " + ", ".join(sorted(unknown))
                )
            if "required" in option and not isinstance(option["required"], bool):
                problems.append(
                    f"{where} (checkboxes): options[{index}].required must be true "
                    "or false"
                )
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

        spec = FORM_TYPES[kind]
        attributes = field.get("attributes")

        if spec["needs_id"]:
            fillable += 1
            field_id = field.get("id")
            if not isinstance(field_id, str) or not FIELD_ID.match(field_id):
                problems.append(
                    f"{where} ({kind}) needs an id of letters, digits, '-' or '_'"
                )
            elif field_id in seen_ids:
                # Two fields sharing an id lose the second one's answer, and the
                # URL prefill for it becomes ambiguous.
                problems.append(f"{where} reuses the id {field_id!r}")
            else:
                seen_ids.add(field_id)
        elif "id" in field:
            problems.append(f"{where} (markdown) takes no id; it submits nothing")

        if attributes is None:
            if spec["needs_id"]:
                problems.append(f"{where} ({kind}) needs attributes")
                continue
        elif not isinstance(attributes, dict):
            problems.append(f"{where} ({kind}): attributes must be a mapping")
        else:
            unknown = set(attributes) - spec["attributes"]
            if unknown:
                # GitHub ignores a key its schema does not list, so the field
                # renders without whatever the author meant to ask for.
                problems.append(
                    f"{where} ({kind}): attribute(s) GitHub does not accept: "
                    + ", ".join(sorted(unknown))
                )
            if spec["needs_id"]:
                label = attributes.get("label")
                if not isinstance(label, str) or not label.strip():
                    problems.append(
                        f"{where} ({kind}) needs attributes.label, the prompt the "
                        "reporter reads"
                    )
                if kind in ("dropdown", "checkboxes"):
                    problems.extend(options_of(where, kind, attributes))

        validations = field.get("validations")
        if validations is not None:
            if not isinstance(validations, dict):
                problems.append(f"{where}: validations must be a mapping")
            else:
                for key, value in sorted(validations.items()):
                    if key not in spec["validations"]:
                        problems.append(
                            f"{where} ({kind}): validation {key!r} is not part of "
                            "that field type; GitHub would ignore it"
                        )
                    elif key == "required":
                        if not isinstance(value, bool):
                            problems.append(
                                f"{where}: validations.required must be true or false"
                            )
                    elif key == "min_length":
                        # A non-empty floor on a text answer: `required` alone lets
                        # a reporter submit "." and satisfy the form.
                        if isinstance(value, bool) or not isinstance(value, int) or value < 0:
                            problems.append(
                                f"{where}: validations.min_length must be a "
                                "non-negative integer"
                            )

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
            "needs a non-empty attributes.options",
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

    def test_min_length_is_accepted(self):
        # `required` only proves a box is not empty; a length floor is how the
        # forms ask for a reproduction worth reading.
        form = GOOD_FORM.replace(
            "    validations:\n      required: true",
            "    validations:\n      required: true\n      min_length: 20",
        )
        self.assertIn("min_length", form)
        self.assertEqual(
            problems_in({"config.yml": GOOD_CONFIG, "bug_report.yml": form}), []
        )

    def test_a_validation_the_type_does_not_take_is_rejected(self):
        # `min_length` on a dropdown is silently dropped by GitHub, so the form
        # would look validated and submit an empty answer.
        form = GOOD_FORM.replace(
            "  - type: input\n    id: version\n    attributes:\n"
            "      label: karucode version\n"
            "      description: Output of `karucode --version`.\n"
            "    validations:\n      required: true\n",
            "  - type: dropdown\n    id: version\n    attributes:\n"
            "      label: karucode version\n      options:\n        - 1\n        - 2\n"
            "    validations:\n      min_length: 3\n",
        )
        self.assert_problem(
            {"config.yml": GOOD_CONFIG, "bug_report.yml": form},
            "'min_length' is not part of that field type",
        )

    def test_an_attribute_the_type_does_not_take_is_rejected(self):
        form = GOOD_FORM.replace(
            "      description: Output of `karucode --version`.\n",
            "      description: Output of `karucode --version`.\n"
            "      options:\n        - a\n",
        )
        self.assert_problem(
            {"config.yml": GOOD_CONFIG, "bug_report.yml": form},
            "attribute(s) GitHub does not accept: options",
        )

    def test_a_repeated_dropdown_option_is_rejected(self):
        form = GOOD_FORM.replace(
            "  - type: input\n    id: version\n    attributes:\n"
            "      label: karucode version\n"
            "      description: Output of `karucode --version`.\n"
            "    validations:\n      required: true\n",
            "  - type: dropdown\n    id: version\n    attributes:\n"
            "      label: karucode version\n"
            "      options:\n        - \"107\"\n        - \"107\"\n",
        )
        self.assert_problem(
            {"config.yml": GOOD_CONFIG, "bug_report.yml": form},
            "repeated option(s): 107",
        )

    def test_a_dropdown_default_outside_its_options_is_rejected(self):
        form = GOOD_FORM.replace(
            "  - type: input\n    id: version\n    attributes:\n"
            "      label: karucode version\n"
            "      description: Output of `karucode --version`.\n"
            "    validations:\n      required: true\n",
            "  - type: dropdown\n    id: version\n    attributes:\n"
            "      label: karucode version\n"
            "      options:\n        - 107\n        - 108\n      default: 5\n",
        )
        self.assert_problem(
            {"config.yml": GOOD_CONFIG, "bug_report.yml": form},
            "attributes.default must be an index",
        )

    def test_a_checkbox_option_without_a_label_is_rejected(self):
        form = GOOD_FORM.replace(
            "  - type: input\n    id: version\n    attributes:\n"
            "      label: karucode version\n"
            "      description: Output of `karucode --version`.\n"
            "    validations:\n      required: true\n",
            "  - type: checkboxes\n    id: version\n    attributes:\n"
            "      label: Agreement\n      options:\n        - required: true\n",
        )
        self.assert_problem(
            {"config.yml": GOOD_CONFIG, "bug_report.yml": form},
            "options[1] needs a label",
        )

    def test_a_markdown_element_cannot_carry_an_id(self):
        # Nothing is submitted from markdown, so an id there is a field the
        # author believes they are collecting.
        form = GOOD_FORM.replace(
            "  - type: markdown\n    attributes:\n",
            "  - type: markdown\n    id: intro\n    attributes:\n",
        )
        self.assert_problem(
            {"config.yml": GOOD_CONFIG, "bug_report.yml": form},
            "(markdown) takes no id",
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