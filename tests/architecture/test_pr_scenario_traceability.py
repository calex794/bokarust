"""Positive and negative checks for PR -> Notion traceability gate."""
from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools"))

import check_pr_scenario as guard  # noqa: E402


SCENARIO = "https://app.notion.com/p/3f632a84beec819ca07ec2dc64df2af5"
TASK = "https://app.notion.com/p/3f632a84beec81bc9d39c878c5c0b715"
REGISTRY = "https://app.notion.com/p/970f1eab10e041489e285725a697b513"

VALID = f"""## Тип изменения
- [x] Пользовательский сценарий
- [ ] Техническая задача

## Сценарии в Notion
- {SCENARIO}

## Техническая задача в Notion
-

## Старое поведение
В Booka есть вход через форму с проверкой пароля.

## Новое поведение и отличия
На Rust добавлена защищённая форма, ограничены запросы.

## Проверки
cargo test --workspace: 9 тестов успешно.

## Что осталось
Нет
"""


class ScenarioTraceabilityTest(unittest.TestCase):
    def test_valid_user_scenario(self) -> None:
        self.assertEqual(guard.validate(VALID), [])

    def test_valid_technical_task_without_old_behavior(self) -> None:
        body = (VALID
                .replace("- [x] Пользовательский сценарий",
                         "- [ ] Пользовательский сценарий")
                .replace("- [ ] Техническая задача",
                         "- [x] Техническая задача")
                .replace("## Техническая задача в Notion\n-",
                         f"## Техническая задача в Notion\n- {TASK}")
                .replace("В Booka есть вход через форму с проверкой пароля.", ""))
        self.assertEqual(guard.validate(body), [])

    def test_empty_body_fails(self) -> None:
        self.assertNotEqual(guard.validate(None), [])
        self.assertNotEqual(guard.validate(""), [])

    def test_missing_scenario_is_blocked(self) -> None:
        body = VALID.replace(SCENARIO, "")
        self.assertTrue(any("карточку сценария" in e for e in guard.validate(body)))

    def test_database_url_is_not_a_scenario(self) -> None:
        body = VALID.replace(SCENARIO, REGISTRY)
        self.assertTrue(any("карточку сценария" in e for e in guard.validate(body)))

    def test_project_plan_is_not_a_scenario(self) -> None:
        body = VALID.replace(
            SCENARIO, "https://app.notion.com/p/3f632a84beec8109a1fde45714cc900e")
        self.assertTrue(any("карточку сценария" in e for e in guard.validate(body)))

    def test_fake_notion_host_is_rejected(self) -> None:
        body = VALID.replace(SCENARIO, SCENARIO.replace(
            "app.notion.com/", "app.notion.com.evil.example/"))
        self.assertTrue(any("карточку сценария" in e for e in guard.validate(body)))

    def test_comments_cannot_smuggle_acceptable_link(self) -> None:
        body = VALID.replace(SCENARIO, f"<!-- {SCENARIO} -->")
        self.assertTrue(any("карточку сценария" in e for e in guard.validate(body)))

    def test_two_selected_types_fail(self) -> None:
        body = VALID.replace("- [ ] Техническая задача", "- [x] Техническая задача")
        self.assertTrue(any("ровно у одного типа" in e for e in guard.validate(body)))

    def test_none_selected_fails(self) -> None:
        body = VALID.replace("- [x] Пользовательский сценарий",
                             "- [ ] Пользовательский сценарий")
        self.assertTrue(any("ровно у одного типа" in e for e in guard.validate(body)))

    def test_missing_old_behavior_blocks_user_changes(self) -> None:
        body = VALID.replace(
            "В Booka есть вход через форму с проверкой пароля.", "")
        self.assertTrue(any("старое поведение" in e for e in guard.validate(body)))

    def test_unfilled_tests_are_rejected(self) -> None:
        body = VALID.replace("cargo test --workspace: 9 тестов успешно.", "TODO")
        self.assertTrue(any("проверки" in e.lower() for e in guard.validate(body)))

    def test_missing_remaining_work_is_rejected(self) -> None:
        body = VALID.replace("## Что осталось\nНет", "## Что осталось\n-")
        self.assertTrue(any("остались" in e for e in guard.validate(body)))

    def test_duplicate_section_is_rejected(self) -> None:
        self.assertTrue(any(
            "Повторяется" in e for e in guard.validate(
                VALID + "\n## Проверки\nНе имеет значения.\n")))

    def test_valid_notion_so_hyphen_uuid(self) -> None:
        uuid = "3f632a84-beec-819c-a07e-c2dc64df2af5"
        body = VALID.replace(SCENARIO, f"https://www.notion.so/Registration-{uuid}")
        self.assertEqual(guard.validate(body), [])

    def test_technical_change_requires_task_page(self) -> None:
        body = (VALID.replace("- [x] Пользовательский сценарий",
                              "- [ ] Пользовательский сценарий")
                .replace("- [ ] Техническая задача", "- [x] Техническая задача"))
        self.assertTrue(any("техническую задачу" in e for e in guard.validate(body)))

    def test_empty_template_fails(self) -> None:
        template = Path(__file__).resolve().parents[2] / ".github/PULL_REQUEST_TEMPLATE.md"
        errors = guard.validate(template.read_text(encoding="utf-8"))
        self.assertTrue(errors)

    def test_event_json_is_read_as_data(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "event.json"
            path.write_text(json.dumps({"pull_request": {"body": VALID}}), encoding="utf-8")
            self.assertEqual(guard.main(["--event", str(path)]), 0)
            path.write_text(json.dumps({"pull_request": {"body": None}}), encoding="utf-8")
            self.assertEqual(guard.main(["--event", str(path)]), 1)
            path.write_text("not json", encoding="utf-8")
            self.assertEqual(guard.main(["--event", str(path)]), 2)

    def test_oversized_description_is_rejected(self) -> None:
        self.assertTrue(any("слишком длинное" in e for e in guard.validate("x" * 100_001)))


if __name__ == "__main__":
    unittest.main()
