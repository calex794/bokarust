#!/usr/bin/env python3
"""Validate PR descriptions against the Booka -> Rust migration registry.

Local format check only: a URL alone does not prove Notion database membership
or that the linked scenarios and tests are actually complete.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path
from urllib.parse import urlsplit


REGISTRY_ID = "970f1eab10e041489e285725a697b513"
GENERAL_PAGE_IDS = {
    REGISTRY_ID,
    "3f632a84beec8109a1fde45714cc900e",  # Project plan
    "e47423d0edc54f25a94a9ab7f33eeadc",  # Work board
}
NOTION_HOSTS = {"notion.so", "www.notion.so", "app.notion.com"}
REQUIRED_SECTIONS = {
    "Тип изменения",
    "Сценарии в Notion",
    "Техническая задача в Notion",
    "Старое поведение",
    "Новое поведение и отличия",
    "Проверки",
    "Что осталось",
}
TYPE_OPTIONS = {"Пользовательский сценарий", "Техническая задача"}
BLANK_VALUES = {
    "", "-", "—", ".", "...", "…", "todo", "tbd", "заполнить",
    "заполню позже", "укажите здесь", "пример", "нет данных",
}


def _without_comments(body: str) -> str:
    return re.sub(r"<!--.*?-->", "", body, flags=re.DOTALL)


def sections(body: str) -> tuple[dict[str, str], list[str]]:
    headings = list(re.finditer(r"(?m)^##[ \t]+([^\n]+)[ \t]*$", body))
    parts: dict[str, str] = {}
    errors: list[str] = []
    for i, heading in enumerate(headings):
        name = heading.group(1).strip()
        ending = headings[i + 1].start() if i + 1 < len(headings) else len(body)
        if name in parts:
            errors.append(f"Повторяется раздел «{name}».")
        else:
            parts[name] = body[heading.end():ending].strip()
    for name in sorted(REQUIRED_SECTIONS - parts.keys()):
        errors.append(f"Нет раздела «{name}» из шаблона PR.")
    return parts, errors


def _is_filled(value: str, *, allow_none: bool = False) -> bool:
    lines = []
    for raw in value.splitlines():
        cleaned = re.sub(r"^\s*[-*+]\s*(?:\[[ xX]\]\s*)?", "", raw).strip()
        cleaned = cleaned.replace(chr(96), "").strip()
        if cleaned and cleaned.casefold() not in BLANK_VALUES:
            lines.append(cleaned)
    text = " ".join(lines).strip()
    if allow_none and text.casefold() in {"нет", "не осталось", "ничего"}:
        return True
    if text.casefold() in {"нет", "не осталось", "ничего"}:
        return False
    prose = re.sub(r"https?://\S+", "", text).strip(" []()<>-—.,:")
    return len(prose) >= 8


def _notion_id(url: str) -> str | None:
    try:
        parsed = urlsplit(url.strip().rstrip(".,;:"))
    except ValueError:
        return None
    if parsed.scheme != "https" or parsed.hostname not in NOTION_HOSTS:
        return None
    path = parsed.path.rstrip("/").split("/")[-1]
    match = re.search(
        r"([0-9a-fA-F]{32}|[0-9a-fA-F]{8}(?:-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12})$",
        path,
    )
    if not match:
        return None
    identifier = match.group(1).replace("-", "").lower()
    if identifier in GENERAL_PAGE_IDS:
        return None
    return identifier


def notion_page_links(section: str) -> list[str]:
    links: list[str] = []
    for match in re.finditer(r"https?://[^\s<>)\]\"']+", section):
        candidate = match.group().rstrip(".,;:")
        if _notion_id(candidate):
            links.append(candidate)
    return list(dict.fromkeys(links))


def validate(body: str | None) -> list[str]:
    if not isinstance(body, str) or not body.strip():
        return ["Описание PR пустое. Заполните шаблон запроса на объединение."]
    if len(body) > 100_000:
        return ["Описание PR слишком длинное: максимально 100 000 символов."]
    parsed, errors = sections(_without_comments(body))

    selected: list[str] = []
    for option in TYPE_OPTIONS:
        found = re.findall(
            r"(?mi)^\s*[-*]\s*\[\s*([xX ])\s*\]\s*"
            + re.escape(option) + r"\s*$",
            parsed.get("Тип изменения", ""),
        )
        if len(found) != 1:
            errors.append(f"В разделе «Тип изменения» должна быть одна строка «{option}».")
        elif found[0].lower() == "x":
            selected.append(option)
    if len(selected) != 1:
        errors.append("Выберите [x] ровно у одного типа: сценарий или техническая задача.")
        return errors

    if selected[0] == "Пользовательский сценарий":
        if not notion_page_links(parsed.get("Сценарии в Notion", "")):
            errors.append(
                "Добавьте ссылку на карточку сценария из реестра Notion "
                "в раздел «Сценарии в Notion» (ссылка на всю базу не подходит)."
            )
        if not _is_filled(parsed.get("Старое поведение", "")):
            errors.append("Опишите старое поведение Booka или отсутствие такой функции.")
    else:
        if not notion_page_links(parsed.get("Техническая задача в Notion", "")):
            errors.append("Добавьте ссылку на конкретную техническую задачу Notion.")

    if not _is_filled(parsed.get("Новое поведение и отличия", "")):
        errors.append("Опишите новое поведение, изменения или причину технической работы.")
    if not _is_filled(parsed.get("Проверки", "")):
        errors.append("Укажите выполненные проверки либо причину, почему они не запускались.")
    if not _is_filled(parsed.get("Что осталось", ""), allow_none=True):
        errors.append("Укажите оставшиеся ограничения или напишите «Нет».")
    return errors


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--event", type=Path, help="GitHub event payload JSON")
    source.add_argument("--body-file", type=Path, help="local PR body for development")
    args = parser.parse_args(argv)
    try:
        if args.event is not None:
            payload = json.loads(args.event.read_text(encoding="utf-8"))
            pull_request = payload.get("pull_request")
            if not isinstance(pull_request, dict):
                raise ValueError("event payload has no pull_request object")
            body = pull_request.get("body")
        else:
            body = args.body_file.read_text(encoding="utf-8")
        errors = validate(body)
    except (OSError, UnicodeError, ValueError, TypeError) as exc:
        print(f"Проверка связи PR не выполнена: {exc}", file=sys.stderr)
        return 2

    if errors:
        print("Проверка соответствия PR → Notion не пройдена:", file=sys.stderr)
        for error in errors:
            print(f" - {error}", file=sys.stderr)
        return 1
    print("Проверка соответствия PR → Notion пройдена (формат ссылок и разделы).")
    print("Принадлежность карточек нужной базе и выполнение тестов не проверяются.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
