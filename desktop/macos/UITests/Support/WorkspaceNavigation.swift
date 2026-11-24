import XCTest

extension XCUIApplication {
  func selectWorkspace(_ destination: String) {
    let showSidebar = buttons["Show Sidebar"]
    if showSidebar.exists { showSidebar.click() }
    let row = descendants(matching: .any)
      .matching(identifier: "navigation-\(destination)").firstMatch
    XCTAssertTrue(row.waitForExistence(timeout: 5))
    row.click()
  }
}
