/**
 * OnboardingStore — persists first-run onboarding completion state.
 */

import { create } from "zustand";

const STORAGE_KEY = "deepdepcat.onboardingComplete";

interface OnboardingState {
  completed: boolean;
  setCompleted: () => void;
  reset: () => void;
}

function loadCompleted(): boolean {
  try {
    return localStorage.getItem(STORAGE_KEY) === "true";
  } catch {
    return false;
  }
}

export const useOnboardingStore = create<OnboardingState>((set) => ({
  completed: loadCompleted(),

  setCompleted: () => {
    try {
      localStorage.setItem(STORAGE_KEY, "true");
    } catch {
      // Ignore — localStorage may be unavailable
    }
    set({ completed: true });
  },

  reset: () => {
    try {
      localStorage.removeItem(STORAGE_KEY);
    } catch {
      // Ignore
    }
    set({ completed: false });
  },
}));
