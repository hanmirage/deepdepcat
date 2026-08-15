/**
 * ProductMenu tests — title-bar surface switcher.
 */

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ProductMenu } from "@/components/layout/ProductMenu";
import { useAppStore } from "@/stores/appStore";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
        "layout.codeMode": "DeepDepCat — 代码",
        "layout.depworkMode": "DeepDepCat — 文档",
        "layout.productCode": "Code",
        "layout.productDepwork": "Depwork",
        "layout.productCodeDesc": "编码助手",
        "layout.productDepworkDesc": "文档办公",
        "layout.productStreaming": "运行中",
      })[key] ?? key,
  }),
}));

describe("ProductMenu", () => {
  beforeEach(() => {
    useAppStore.setState({ mode: "code" });
  });

  it("shows the current product as the trigger label", () => {
    render(<ProductMenu />);

    expect(
      screen.getByRole("button", { name: "DeepDepCat — 代码" }),
    ).toBeInTheDocument();
  });

  it("switches surfaces from the menu", async () => {
    const user = userEvent.setup();
    render(<ProductMenu />);

    await user.click(
      screen.getByRole("button", { name: "DeepDepCat — 代码" }),
    );
    await user.click(await screen.findByText("Depwork"));

    expect(useAppStore.getState().mode).toBe("depwork");
  });

  it("marks streaming surfaces with a live dot", async () => {
    const user = userEvent.setup();
    render(<ProductMenu codeStreaming depworkStreaming />);

    await user.click(
      screen.getByRole("button", { name: "DeepDepCat — 代码" }),
    );
    const codeItem = await screen.findByText("Code");
    const depworkItem = screen.getByText("Depwork");

    expect(codeItem.closest("[data-mode]")).toHaveAttribute(
      "data-streaming",
      "true",
    );
    expect(depworkItem.closest("[data-mode]")).toHaveAttribute(
      "data-streaming",
      "true",
    );
  });
});
