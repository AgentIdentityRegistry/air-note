import { createContext, useContext, useReducer, ReactNode, Dispatch } from "react";

export type OnboardingStep =
  | "welcome"
  | "name"
  | "generating"
  | "registering"
  | "done";

export type OnboardingState = {
  step: OnboardingStep;
  name: string;
  domain: string;
  error: string | null;
};

type OnboardingAction =
  | { type: "next" }
  | { type: "back" }
  | { type: "set_name"; name: string }
  | { type: "set_domain"; domain: string }
  | { type: "error"; message: string }
  | { type: "reset" };

const initial: OnboardingState = {
  step: "welcome",
  name: "",
  domain: "bossclaw.ai",
  error: null,
};

function reduce(s: OnboardingState, a: OnboardingAction): OnboardingState {
  switch (a.type) {
    case "next":
      return { ...s, step: nextStep(s.step), error: null };
    case "back":
      return { ...s, step: prevStep(s.step), error: null };
    case "set_name":
      return { ...s, name: a.name };
    case "set_domain":
      return { ...s, domain: a.domain };
    case "error":
      return { ...s, error: a.message };
    case "reset":
      return initial;
  }
}

function nextStep(s: OnboardingStep): OnboardingStep {
  const order: OnboardingStep[] = ["welcome", "name", "generating", "registering", "done"];
  const i = order.indexOf(s);
  return order[Math.min(i + 1, order.length - 1)];
}

function prevStep(s: OnboardingStep): OnboardingStep {
  const order: OnboardingStep[] = ["welcome", "name", "generating", "registering", "done"];
  const i = order.indexOf(s);
  return order[Math.max(i - 1, 0)];
}

const Ctx = createContext<{
  state: OnboardingState;
  dispatch: Dispatch<OnboardingAction>;
} | null>(null);

export function OnboardingProvider({ children }: { children: ReactNode }) {
  const [state, dispatch] = useReducer(reduce, initial);
  return <Ctx.Provider value={{ state, dispatch }}>{children}</Ctx.Provider>;
}

export function useOnboarding() {
  const c = useContext(Ctx);
  if (!c) throw new Error("useOnboarding must be inside OnboardingProvider");
  return c;
}
