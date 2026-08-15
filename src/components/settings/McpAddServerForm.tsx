/**
 * McpAddServerForm — inline form for adding a new MCP server.
 *
 * Fields:
 * - Server name (required)
 * - Transport type: stdio | http | sse
 * - For stdio: command + args + env
 * - For http/sse: url
 *
 * On submit, calls `onAdd` with the configured McpServerConfig.
 */

import { useState, useCallback } from "react";
import { useTranslation } from "react-i18next";
import { Plus, ChevronDown } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { SettingSelect } from "@/components/settings/SettingSelect";
import type { McpTransportType } from "@/types";

export interface McpAddServerFormProps {
  onAdd: (config: {
    name: string;
    type: McpTransportType;
    command: string | null;
    args: string[];
    env: Record<string, string>;
    url: string | null;
    enabled: boolean;
  }) => void;
}

export function McpAddServerForm({ onAdd }: McpAddServerFormProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const [transport, setTransport] = useState<McpTransportType>("stdio");
  const [command, setCommand] = useState("");
  const [args, setArgs] = useState("");
  const [url, setUrl] = useState("");

  /** One-click presets — prefill the form for known bundled servers. */
  const PRESETS: {
    key: string;
    labelKey: string;
    type: McpTransportType;
    command: string;
    args: string[];
    url: string | null;
  }[] = [
    {
      key: "wps",
      labelKey: "settings.mcp.presetWps",
      type: "stdio",
      command: "python",
      args: ["-m", "wps_controller.mcp_server"],
      url: null,
    },
  ];

  const applyPreset = (preset: (typeof PRESETS)[number]) => {
    setName(preset.key === "wps" ? "wps-office" : preset.key);
    setTransport(preset.type);
    setCommand(preset.command);
    setArgs(preset.args.join(" "));
    setUrl(preset.url ?? "");
  };

  const isStdio = transport === "stdio";
  const canSubmit = name.trim().length > 0 && (isStdio ? command.trim().length > 0 : url.trim().length > 0);

  /** Split args on whitespace, respecting double quotes — an arg like
   *  `"C:\My Folder\server"` must stay one token, not two. */
  const parseArgs = (raw: string): string[] => {
    const tokens: string[] = [];
    const re = /"([^"]*)"|'([^']*)'|(\S+)/g;
    let m: RegExpExecArray | null;
    while ((m = re.exec(raw)) !== null) {
      tokens.push(m[1] ?? m[2] ?? m[3]);
    }
    return tokens;
  };

  const handleSubmit = useCallback(() => {
    if (!canSubmit) return;
    onAdd({
      name: name.trim(),
      type: transport,
      command: isStdio ? command.trim() : null,
      args: isStdio ? parseArgs(args) : [],
      env: {},
      url: isStdio ? null : url.trim(),
      enabled: true,
    });
    setName("");
    setCommand("");
    setArgs("");
    setUrl("");
    setOpen(false);
  }, [canSubmit, name, transport, command, args, url, isStdio, onAdd]);

  return (
    <Collapsible open={open} onOpenChange={setOpen}>
      <CollapsibleTrigger asChild>
        <Button variant="outline" size="sm" className="h-8 gap-1 text-xs">
          {open ? <ChevronDown className="h-3.5 w-3.5" /> : <Plus className="h-3.5 w-3.5" />}
          {t("settings.mcp.addServer")}
        </Button>
      </CollapsibleTrigger>
      <CollapsibleContent>
        <div className="mt-3 space-y-3 rounded-lg border border-border bg-card p-3">
          {/* Presets — one-click prefill for bundled servers */}
          {PRESETS.length > 0 && (
            <div className="flex flex-wrap items-center gap-2">
              <span className="text-[10px] font-medium text-muted-foreground">
                {t("settings.mcp.presets")}
              </span>
              {PRESETS.map((preset) => (
                <Button
                  key={preset.key}
                  variant="outline"
                  size="sm"
                  className="h-7 gap-1 text-[11px]"
                  onClick={() => applyPreset(preset)}
                >
                  <Plus className="h-3 w-3" />
                  {t(preset.labelKey)}
                </Button>
              ))}
            </div>
          )}

          {/* Name + transport */}
          <div className="flex gap-3">
            <div className="flex-1">
              <label className="mb-1 block text-[10px] font-medium text-muted-foreground">
                {t("settings.mcp.serverName")}
              </label>
              <Input
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="filesystem"
                className="h-8 text-xs"
              />
            </div>
            <div className="w-32">
              <label className="mb-1 block text-[10px] font-medium text-muted-foreground">
                {t("settings.mcp.transport")}
              </label>
              <SettingSelect
                value={transport}
                onChange={(v) => setTransport(v as McpTransportType)}
                options={[
                  { value: "stdio", label: "stdio" },
                  { value: "http", label: "HTTP" },
                  { value: "sse", label: "SSE" },
                ]}
                className="w-full"
              />
            </div>
          </div>

          {/* stdio fields */}
          {isStdio ? (
            <>
              <div>
                <label className="mb-1 block text-[10px] font-medium text-muted-foreground">
                  {t("settings.mcp.command")}
                </label>
                <Input
                  value={command}
                  onChange={(e) => setCommand(e.target.value)}
                  placeholder="npx"
                  className="h-8 text-xs"
                />
              </div>
              <div>
                <label className="mb-1 block text-[10px] font-medium text-muted-foreground">
                  {t("settings.mcp.args")}
                </label>
                <Input
                  value={args}
                  onChange={(e) => setArgs(e.target.value)}
                  placeholder="-y @modelcontextprotocol/server-filesystem /path"
                  className="h-8 text-xs"
                />
              </div>
            </>
          ) : (
            <div>
              <label className="mb-1 block text-[10px] font-medium text-muted-foreground">
                URL
              </label>
              <Input
                value={url}
                onChange={(e) => setUrl(e.target.value)}
                placeholder="https://example.com/mcp"
                className="h-8 text-xs"
              />
            </div>
          )}

          {/* Submit */}
          <div className="flex justify-end">
            <Button
              size="sm"
              className="h-8 text-xs"
              disabled={!canSubmit}
              onClick={handleSubmit}
            >
              {t("common.add")}
            </Button>
          </div>
        </div>
      </CollapsibleContent>
    </Collapsible>
  );
}
