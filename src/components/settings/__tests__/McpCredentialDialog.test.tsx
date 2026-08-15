import { describe, it, expect, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { McpCredentialDialog } from "@/components/settings/McpCredentialDialog";
import type { McpServerConfig } from "@/types";

vi.mock("react-i18next", () => ({
  initReactI18next: { type: "3rdParty", init: () => {} },
  useTranslation: () => ({
    t: (key: string, opts?: { defaultValue?: string }) => opts?.defaultValue ?? key,
  }),
}));

const server: McpServerConfig = {
  name: "srv",
  type: "http",
  command: null,
  args: [],
  env: {},
  url: "https://srv.example",
  enabled: true,
};

describe("McpCredentialDialog", () => {
  it("saves the credential with renewal fields", async () => {
    const onSave = vi.fn(async () => {});
    render(
      <McpCredentialDialog
        server={server}
        open
        onOpenChange={() => {}}
        onSave={onSave}
        onDelete={vi.fn(async () => {})}
        hasCredential={false}
      />,
    );

    fireEvent.change(screen.getByPlaceholderText("https://example.com/oauth/token"), {
      target: { value: "https://srv.example/oauth/token" },
    });
    fireEvent.change(screen.getByPlaceholderText("optional"), {
      target: { value: "client-1" },
    });
    fireEvent.change(screen.getByLabelText("Access Token"), {
      target: { value: "tok" },
    });
    fireEvent.change(screen.getByLabelText("Refresh Token（可选）"), {
      target: { value: "refresh" },
    });
    fireEvent.click(screen.getByText("common.save"));

    expect(onSave).toHaveBeenCalledWith({
      tokenEndpoint: "https://srv.example/oauth/token",
      clientId: "client-1",
      accessToken: "tok",
      refreshToken: "refresh",
      tokenType: "Bearer",
      expiresAt: "",
    });
  });

  it("requires an access token", async () => {
    const onSave = vi.fn(async () => {});
    render(
      <McpCredentialDialog
        server={server}
        open
        onOpenChange={() => {}}
        onSave={onSave}
        onDelete={vi.fn(async () => {})}
        hasCredential={false}
      />,
    );

    fireEvent.click(screen.getByText("common.save"));
    expect(screen.getByText("访问令牌必填")).toBeTruthy();
    expect(onSave).not.toHaveBeenCalled();
  });

  it("deletes the stored credential", async () => {
    const onDelete = vi.fn(async () => {});
    render(
      <McpCredentialDialog
        server={server}
        open
        onOpenChange={() => {}}
        onSave={vi.fn(async () => {})}
        onDelete={onDelete}
        hasCredential
      />,
    );

    fireEvent.click(screen.getByText("删除凭据"));
    expect(onDelete).toHaveBeenCalled();
  });
});
