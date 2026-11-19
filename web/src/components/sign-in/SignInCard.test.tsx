import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { expectTrustWording } from "../../__tests__/wording";
import { SignInCard } from "./SignInCard";

describe("SignInCard", () => {
  it("has a primary 'Sign in with GitHub' action with a tooltip, the purpose and the no-submit promise", () => {
    render(<SignInCard loginHref="/auth/github/login" />);
    const signIn = screen.getByRole("link", { name: "Sign in with GitHub" });
    expect(signIn).toHaveAttribute("href", "/auth/github/login");
    expect(signIn).toHaveAttribute("data-variant", "primary");
    expect(signIn).toHaveAccessibleDescription(/GitHub/);
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("smart-review");
    expect(document.body).toHaveTextContent("It never submits a review for you.");
    expectTrustWording(document.body.textContent ?? "");
  });
});
