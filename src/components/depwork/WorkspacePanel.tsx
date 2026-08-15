/**
 * WorkspacePanel — depwork workspace browser in the right panel.
 *
 * A direct-pane layout (no card nesting): a "工作区" section header, the file
 * tree, and — when a file is selected — a compact preview below it. The
 * preview supports full-screen expand (FilePreviewDialog), so the embedded
 * height stays small. Reads depworkStore like the FileTree.
 */

import { FolderOpen } from "lucide-react";
import { useTranslation } from "react-i18next";
import { FileTree } from "@/components/depwork/FileTree";
import { PreviewPanel } from "@/components/depwork/PreviewPanel";
import { SectionHeader } from "@/components/customize/panelParts";
import { useDepworkStore } from "@/stores/depworkStore";

export function WorkspacePanel() {
  const { t } = useTranslation();
  const selectedFile = useDepworkStore((s) => s.selectedFile);

  return (
    <div className="space-y-2">
      <SectionHeader
        icon={FolderOpen}
        label={t("depwork.workspace", { defaultValue: "工作区" })}
      />
      <FileTree />
      {selectedFile && <PreviewPanel className="h-72" />}
    </div>
  );
}
