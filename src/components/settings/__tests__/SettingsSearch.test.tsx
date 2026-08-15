import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Eye, Palette } from "lucide-react";
import { SettingsSearchNav } from "@/components/settings/SettingsSearch";
import type { SettingsSearchResult } from "@/config/settingsSearch";

class ResizeObserverStub {
  observe() {}
  unobserve() {}
  disconnect() {}
}

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, options?: { count?: number }) =>
      ({
        "settings.search.placeholder": "搜索设置",
        "settings.search.empty": "没有找到匹配的设置",
        "settings.search.emptyHint": "试试设置名称或描述中的关键词",
        "settings.search.results": `${options?.count ?? 0} 个结果`,
        "common.clear": "清空",
        "settingsGroups.config": "配置",
        "settingsCategories.appearance": "外观与行为",
        "settingsCategories.models": "模型",
      })[key] ?? key,
  }),
}));

beforeEach(() => {
  vi.stubGlobal("ResizeObserver", ResizeObserverStub);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

const RESULT: SettingsSearchResult = {
  category: {
    id: "appearance",
    label: "settingsCategories.appearance",
    icon: Palette,
  },
  groupLabel: "配置",
  categoryMatched: false,
  entryMatches: [
    {
      entry: { key: "settings.general.theme", descKey: "settings.general.themeDesc" },
      label: "界面主题",
      desc: "切换应用界面使用的主题外观。",
    },
  ],
};

describe("SettingsSearchNav navigation", () => {
  it("shows the grouped nav without a query and the results with one", () => {
    const { rerender } = render(
      <SettingsSearchNav
        query=""
        onQueryChange={() => {}}
        results={[]}
        activeCategory="appearance"
        hideVision={false}
        onSelect={() => {}}
      />,
    );
    expect(screen.getByText("外观与行为")).toBeTruthy();

    rerender(
      <SettingsSearchNav
        query="主题"
        onQueryChange={() => {}}
        results={[RESULT]}
        activeCategory="appearance"
        hideVision={false}
        onSelect={() => {}}
      />,
    );
    expect(screen.getByRole("button", { name: /界面主题/ })).toBeTruthy();
    expect(screen.getByText("1 个结果")).toBeTruthy();
  });

  it("hides the vision category when hideVision is on", () => {
    const visionResult: SettingsSearchResult = {
      ...RESULT,
      category: {
        id: "vision",
        label: "settingsCategories.vision",
        icon: Eye,
      },
    };
    const { rerender } = render(
      <SettingsSearchNav
        query="视觉"
        onQueryChange={() => {}}
        results={[visionResult]}
        activeCategory="vision"
        hideVision
        onSelect={() => {}}
      />,
    );
    expect(screen.getByText("没有找到匹配的设置")).toBeTruthy();
    rerender(
      <SettingsSearchNav
        query="视觉"
        onQueryChange={() => {}}
        results={[visionResult]}
        activeCategory="vision"
        hideVision={false}
        onSelect={() => {}}
      />,
    );
    expect(screen.queryByText("没有找到匹配的设置")).toBeNull();
  });
});

describe("SettingsSearchNav selection", () => {
  it("selects a matched setting and passes its entry key", async () => {
    const onSelect = vi.fn();
    render(
      <SettingsSearchNav
        query="主题"
        onQueryChange={() => {}}
        results={[RESULT]}
        activeCategory="appearance"
        hideVision={false}
        onSelect={onSelect}
      />,
    );
    await userEvent.click(screen.getByRole("button", { name: /界面主题/ }));
    expect(onSelect).toHaveBeenCalledWith("appearance", "settings.general.theme");
  });

  it("jumps to the first result on Enter and clears on Escape", async () => {
    const onSelect = vi.fn();
    const onQueryChange = vi.fn();
    render(
      <SettingsSearchNav
        query="主题"
        onQueryChange={onQueryChange}
        results={[RESULT]}
        activeCategory="appearance"
        hideVision={false}
        onSelect={onSelect}
      />,
    );
    const input = screen.getByLabelText("搜索设置");
    await userEvent.type(input, "{enter}");
    expect(onSelect).toHaveBeenCalledWith("appearance", "settings.general.theme");
    await userEvent.type(input, "{escape}");
    expect(onQueryChange).toHaveBeenCalledWith("");
  });
});
