import { z } from "zod";

export const billingProviderSchema = z.enum(["stripe", "paddle"]);

export type BillingProvider = z.infer<typeof billingProviderSchema>;

export const subscriptionStatusSchema = z.enum([
  "free",
  "trialing",
  "active",
  "past_due",
  "canceled",
  "incomplete",
  "incomplete_expired",
  "unpaid",
  "paused"
]);

export type SubscriptionStatus = z.infer<typeof subscriptionStatusSchema>;

export const userSchema = z.object({
  id: z.string().uuid(),
  email: z.string().email(),
  createdAt: z.string().datetime().optional(),
  subscriptionStatus: subscriptionStatusSchema.optional()
});

export type User = z.infer<typeof userSchema>;

export const authStartInputSchema = z.object({
  email: z.string().email()
});

export type AuthStartInput = z.infer<typeof authStartInputSchema>;

export const authVerifyInputSchema = z.object({
  email: z.string().email(),
  code: z.string().regex(/^\d{6}$/)
});

export type AuthVerifyInput = z.infer<typeof authVerifyInputSchema>;

export const subscriptionSummarySchema = z.object({
  provider: billingProviderSchema,
  active: z.boolean(),
  status: subscriptionStatusSchema,
  currentPeriodEnd: z.string().datetime().nullable(),
  cancelAtPeriodEnd: z.boolean()
});

export type SubscriptionSummary = z.infer<typeof subscriptionSummarySchema>;
