// A flux finding as /api/candidates returns it, for unit tests.

export const G = "6f1c2e3a-0000-4000-8000-000000000001";

export const RAW = {
  id: "c-0a1b2c3d",
  created: "2026-11-23T15:00:00Z",
  updated: "2026-11-23T15:01:00Z",
  flagged_by: "human",
  finding: {
    ruleId: "WSTG-INPV-05",
    level: "error",
    rank: 60,
    message: { text: "SQL injection in sort" },
    fingerprints: { "flux/dast/v1": "ab".repeat(32) },
    locations: [
      {
        physicalLocation: { artifactLocation: { uri: "https://shop.test/api/p/17" } },
        logicalLocations: [{ name: "GET", kind: "httpMethod" }],
      },
    ],
    taxa: [{ id: "89", toolComponent: { name: "CWE" } }, { id: "x", toolComponent: { name: "CWE" } }, { id: "1", toolComponent: { name: "CAPEC" } }],
    webRequest: { method: "GET", target: "https://shop.test/api/p/17?sort=x'" },
    webResponse: { statusCode: 500 },
    properties: { "flux/status": "candidate", "flux/replayIds": ["ex-1", "ex-2", 3], "spall/parameter": "sort" },
  },
};

