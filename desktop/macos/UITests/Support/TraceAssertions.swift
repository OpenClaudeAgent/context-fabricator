import XCTest

@MainActor
func assertTraceText(_ value: String, identifier: String, in app: XCUIApplication) {
  let element = app.staticTexts[identifier]
  XCTAssertTrue(element.waitForExistence(timeout: 5))
  XCTAssertEqual(traceText(element), value)
}

@MainActor
func traceText(_ element: XCUIElement) -> String {
  let value = element.value as? String ?? ""
  return value.isEmpty ? element.label : value
}

@MainActor
func isElapsedTime(_ value: String) -> Bool {
  [" µs", " ms", " s"].contains { value.hasSuffix($0) }
}
