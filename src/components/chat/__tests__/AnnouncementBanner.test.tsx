import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { AnnouncementBanner } from "@/components/chat/AnnouncementBanner";
import { sendNotification, isPermissionGranted } from "@tauri-apps/plugin-notification";

const fetchSiteConfigMock = vi.fn();
const sendNotificationMock = vi.mocked(sendNotification);
const isPermissionGrantedMock = vi.mocked(isPermissionGranted);

vi.mock("@/lib/tauri/api/identity", () => ({
  cloudApi: { fetchSiteConfig: (...args: unknown[]) => fetchSiteConfigMock(...args) },
}));

vi.mock("@/lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/tauri")>();
  return { ...actual, isTauri: true };
});

vi.mock("@tauri-apps/plugin-notification", () => ({
  isPermissionGranted: vi.fn().mockResolvedValue(true),
  requestPermission: vi.fn().mockResolvedValue("granted"),
  sendNotification: vi.fn(),
}));

vi.mock("@/stores/authStore", () => ({
  useAuthStore: (selector: (s: { serverUrl: string }) => unknown) =>
    selector({ serverUrl: "https://deepdepcat.hsmiai.xyz" }),
}));

const ACTIVE = {
  id: "ann-test-1",
  enabled: true,
  title: "v1.1.7 已发布",
  message: "包含 M1-M6 工业化与 DSML 修复。",
  level: "info" as const,
};

describe("AnnouncementBanner", () => {
  beforeEach(() => {
    fetchSiteConfigMock.mockReset();
    sendNotificationMock.mockClear();
    isPermissionGrantedMock.mockClear();
    localStorage.clear();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("renders an enabled announcement", async () => {
    fetchSiteConfigMock.mockResolvedValue({ announcement: ACTIVE });
    render(<AnnouncementBanner />);
    expect(await screen.findByText(/v1\.1\.7 已发布/)).toBeInTheDocument();
    expect(screen.getByText(/DSML/)).toBeInTheDocument();
  });

  it("stays hidden when the announcement is disabled", async () => {
    fetchSiteConfigMock.mockResolvedValue({
      announcement: { ...ACTIVE, enabled: false },
    });
    render(<AnnouncementBanner />);
    await new Promise((r) => setTimeout(r, 0));
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("remembers dismissal for the same id", async () => {
    fetchSiteConfigMock.mockResolvedValue({ announcement: ACTIVE });
    const { unmount } = render(<AnnouncementBanner />);
    fireEvent.click(await screen.findByLabelText("关闭公告"));
    expect(screen.queryByRole("status")).toBeNull();
    expect(localStorage.getItem("deepdepcat.announcement.dismissed")).toContain("ann-test-1");

    unmount();
    const again = render(<AnnouncementBanner />);
    await new Promise((r) => setTimeout(r, 0));
    expect(again.queryByRole("status")).toBeNull();
  });

  it("refreshes periodically and notifies when a new announcement appears", async () => {
    vi.useFakeTimers();
    fetchSiteConfigMock.mockResolvedValue({ announcement: null });
    render(<AnnouncementBanner />);
    await vi.advanceTimersByTimeAsync(0);
    expect(screen.queryByRole("status")).toBeNull();

    fetchSiteConfigMock.mockResolvedValue({ announcement: ACTIVE });
    await vi.advanceTimersByTimeAsync(5 * 60 * 1000);

    expect(screen.getByText(/v1\.1\.7 已发布/)).toBeInTheDocument();
    expect(sendNotificationMock).toHaveBeenCalledTimes(1);
    expect(sendNotificationMock).toHaveBeenCalledWith(
      expect.objectContaining({ title: "DeepDepCat 公告：v1.1.7 已发布" }),
    );
  });

  it("does not re-notify the same announcement on later refreshes", async () => {
    vi.useFakeTimers();
    fetchSiteConfigMock.mockResolvedValue({ announcement: ACTIVE });
    render(<AnnouncementBanner />);
    await vi.advanceTimersByTimeAsync(0);
    // First load shows the banner WITHOUT a system notification.
    expect(sendNotificationMock).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(5 * 60 * 1000);
    expect(sendNotificationMock).not.toHaveBeenCalled();
    expect(screen.getByRole("status")).toBeInTheDocument();
  });
});
