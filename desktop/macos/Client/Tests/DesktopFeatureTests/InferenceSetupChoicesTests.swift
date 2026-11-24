import Foundation
import LocalAPI
import Testing

@testable import DesktopFeature

private func model(_ profile: String, kind: String, role: String?) throws
  -> Components.Schemas.ModelDownload
{
  var value: [String: Any] = [
    "profile": profile, "kind": kind, "state": "notInstalled", "downloadedBytes": 0,
    "totalBytes": 100,
  ]
  value["setupRole"] = role
  return try JSONDecoder().decode(
    Components.Schemas.ModelDownload.self, from: JSONSerialization.data(withJSONObject: value))
}

@Test
func setupChoicesSupportEmptySingleAndMultipleCompatibleModels() throws {
  let empty = InferenceSetupChoices(profiles: [])
  #expect(empty.defaultAnswer == nil)
  #expect(!empty.contains(answer: "missing", embedding: nil))

  let single = InferenceSetupChoices(profiles: [
    try model("owned", kind: "generation", role: "alternativeAnswer")
  ])
  #expect(single.defaultAnswer == "owned")
  #expect(single.contains(answer: "owned", embedding: nil))
  #expect(!single.contains(answer: "owned", embedding: "missing"))

  let choices = InferenceSetupChoices(profiles: [
    try model("alternative", kind: "generation", role: "alternativeAnswer"),
    try model("recommended", kind: "generation", role: "recommendedAnswer"),
    try model("q8", kind: "embedding", role: "optionalEmbedding"),
    try model("f16", kind: "embedding", role: "optionalEmbedding"),
    try model("unsupported", kind: "generation", role: nil),
  ])
  #expect(choices.defaultAnswer == "recommended")
  #expect(choices.contains(answer: "alternative", embedding: nil))
  #expect(choices.contains(answer: "alternative", embedding: "f16"))
  #expect(!choices.contains(answer: "unsupported", embedding: "q8"))
  #expect(!choices.contains(answer: "alternative", embedding: "removed"))
}

@Test
func setupSelectionRoundTripsExplicitAndLegacyEmbeddingProfiles() throws {
  let selected = Components.Schemas.InferenceSetupSelection(
    answerProfile: "alternative", embedding: true, embeddingProfile: "f16", runtimeSource: .hosted)
  let encoded = try JSONEncoder().encode(selected)
  #expect(
    try JSONDecoder().decode(Components.Schemas.InferenceSetupSelection.self, from: encoded)
      == selected)
  let legacy = Data(#"{"embedding":true,"runtimeSource":"hosted"}"#.utf8)
  let decoded = try JSONDecoder().decode(
    Components.Schemas.InferenceSetupSelection.self, from: legacy)
  #expect(decoded.embedding && decoded.embeddingProfile == nil)
}
