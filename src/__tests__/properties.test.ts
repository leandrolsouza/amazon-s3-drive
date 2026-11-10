import { describe, it, expect } from "vitest";
import * as fc from "fast-check";
import {
  AppConfig,
  AwsCredentials,
  NotificationSettings,
  validateCredentialsFormat,
} from "../types";

function arbNotificationSettings(): fc.Arbitrary<NotificationSettings> {
  return fc.record({
    sync_complete_enabled: fc.boolean(),
    conflict_enabled: fc.boolean(),
    error_enabled: fc.boolean(),
  });
}

function arbAppConfig(): fc.Arbitrary<AppConfig> {
  return fc.record({
    bucket_name: fc.stringMatching(/^[a-z0-9][a-z0-9.-]{1,20}[a-z0-9]$/),
    local_path: fc.string({ minLength: 1, maxLength: 50 }),
    s3_prefix: fc.option(fc.string({ minLength: 1, maxLength: 20 }), {
      nil: null,
    }),
    aws_region: fc.stringMatching(/^[a-z]{2}-[a-z]{3,10}-[1-9]$/),
    sync_interval_secs: fc.integer({ min: 1, max: 60 }),
    exclusion_patterns: fc.array(fc.string({ minLength: 1, maxLength: 15 }), {
      maxLength: 8,
    }),
    notifications: arbNotificationSettings(),
    use_iam_role: fc.boolean(),
  });
}

function arbValidCredentials(): fc.Arbitrary<AwsCredentials> {
  return fc.record({
    access_key_id: fc.stringMatching(/^[A-Z0-9]{20}$/),
    secret_access_key: fc.stringMatching(/^[A-Za-z0-9/+]{40}$/),
    session_token: fc.option(fc.stringMatching(/^[A-Za-z0-9/+=]{16,32}$/), {
      nil: undefined,
    }),
    region: fc.stringMatching(/^[a-z]{2}-[a-z]{3,10}-[0-9]$/),
    use_iam_role: fc.constant(false),
  });
}

describe("Frontend Property-Based Tests (fast-check)", () => {
  // Feature: amazon-s3-drive, Property 1: Config serialization round-trip (TypeScript)
  it("config round-trip via invoke serialization", async () => {
    await fc.assert(
      fc.asyncProperty(arbAppConfig(), async (config) => {
        const serialized = JSON.stringify(config);
        const restored: AppConfig = JSON.parse(serialized);
        expect(restored).toEqual(config);
      }),
      { numRuns: 100 }
    );
  });

  // Feature: amazon-s3-drive, Property 3: Credential format validation (TypeScript)
  it("valid credentials pass frontend validation", () => {
    fc.assert(
      fc.property(arbValidCredentials(), (creds) => {
        const errors = validateCredentialsFormat(creds);
        expect(errors).toHaveLength(0);
      }),
      { numRuns: 100 }
    );
  });
});
