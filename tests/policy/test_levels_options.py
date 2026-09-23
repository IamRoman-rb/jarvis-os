"""La política de niveles y la configuración del SDK no se pueden aflojar sin romper un test."""

import re
import warnings
from pathlib import Path
from typing import Any

import pytest
from claude_agent_sdk import CanUseToolShadowedWarning
from claude_agent_sdk.types import _warn_if_can_use_tool_shadowed

from jarvis.agent.brain import build_options
from jarvis.core.config import JarvisConfig
from jarvis.policy.audit import AuditEvent, AuditLog
from jarvis.policy.levels import TOOL_LEVELS, Level, Origin, UnknownToolError, level_of
from jarvis.tools._common import ok
from jarvis.tools.registry import SPECS, ToolSpec, build_tools
from tests.helpers import FakeConfirmer

PERMISOS_MD = Path(__file__).parents[2] / "docs" / "permisos.md"


async def _noop(args: dict[str, Any], cfg: JarvisConfig) -> dict[str, Any]:
    return ok("")


# --- niveles ----------------------------------------------------------------------------------


def test_tool_sin_nivel_no_se_registra(cfg: JarvisConfig) -> None:
    with pytest.raises(UnknownToolError):
        build_tools(cfg, [ToolSpec("borrar_todo", "sin nivel", {}, _noop)])


def test_cada_tool_registrada_tiene_nivel_y_viceversa() -> None:
    assert {s.name for s in SPECS} == set(TOOL_LEVELS)


@pytest.mark.parametrize(
    "name", ["Bash", "Write", "mcp__otro__trash_file", "mcp__system__inexistente", ""]
)
def test_level_of_default_deny(name: str) -> None:
    with pytest.raises(UnknownToolError):
        level_of(name)


def test_permisos_md_coincide_con_levels_py() -> None:
    """docs/permisos.md es la fuente de verdad: si difiere del código, falla."""
    filas = re.findall(r"^\|\s*`(\w+)`\s*\|\s*(\d)\s*\|", PERMISOS_MD.read_text("utf-8"), re.M)
    assert {name: Level(int(lvl)) for name, lvl in filas} == TOOL_LEVELS


# --- opciones del SDK -------------------------------------------------------------------------


@pytest.fixture
def options(cfg: JarvisConfig, audit: AuditLog) -> Any:
    return build_options(cfg, origin=Origin.LOCAL, confirmer=FakeConfirmer(True), audit=audit)


def test_sin_tools_built_in_ni_settings_externos(options: Any) -> None:
    assert options.tools == []
    assert options.setting_sources == []


def test_permission_mode_seguro(options: Any) -> None:
    assert options.permission_mode == "default"


def test_can_use_tool_y_hook_de_auditoria_presentes(options: Any) -> None:
    assert options.can_use_tool is not None
    [matcher] = options.hooks["PreToolUse"]
    assert matcher.matcher is None  # audita TODAS las tools
    assert matcher.hooks


async def test_hook_audita_sin_decidir(options: Any, audit: AuditLog) -> None:
    [matcher] = options.hooks["PreToolUse"]
    out = await matcher.hooks[0](
        {"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": "ls"}},
        "toolu_1",
        {"signal": None},
    )
    assert out == {}
    [entry] = await audit.entries()
    assert (entry.tool, entry.level, entry.event) == ("Bash", None, AuditEvent.REQUESTED)


def test_nada_se_autoaprueba_fuera_de_la_compuerta(options: Any) -> None:
    """Con allowed_tools vacío, toda tool pasa por can_use_tool (y el SDK no avisa nada)."""
    assert options.allowed_tools == []
    with warnings.catch_warnings():
        warnings.simplefilter("error", CanUseToolShadowedWarning)
        _warn_if_can_use_tool_shadowed(options)
