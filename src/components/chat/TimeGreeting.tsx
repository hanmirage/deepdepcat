/**
 * TimeGreeting — time-aware greeting message.
 *
 * Returns a greeting based on the current hour, styled as a
 * large gradient text heading.
 */

import { useMemo } from "react";
import { useTranslation } from "react-i18next";

function getGreetingKey(hour: number): string {
  if (hour >= 0 && hour < 6) return "timeGreeting.night";
  if (hour >= 6 && hour < 9) return "timeGreeting.morning";
  if (hour >= 9 && hour < 12) return "timeGreeting.forenoon";
  if (hour >= 12 && hour < 14) return "timeGreeting.noon";
  if (hour >= 14 && hour < 18) return "timeGreeting.afternoon";
  if (hour >= 18 && hour < 23) return "timeGreeting.evening";
  return "timeGreeting.night";
}

export function TimeGreeting() {
  const { t } = useTranslation();
  const greeting = useMemo(() => {
    const now = new Date();
    return t(getGreetingKey(now.getHours()));
  }, [t]);

  return (
    <h2 className="bg-gradient-to-r from-foreground to-foreground/60 bg-clip-text text-center text-xl font-semibold text-transparent">
      {greeting}
    </h2>
  );
}
