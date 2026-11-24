import XCTest

extension XCUIApplication {
  func selectWorkspace(_ destination: String) {
    let row = descendants(matching: .any)
      .matching(identifier: "navigation-\(destination)").firstMatch
    let showSidebar = buttons["Show Sidebar"]
    if !row.isHittable && showSidebar.isHittable { showSidebar.click() }
    guard row.wait(for: \.isHittable, toEqual: true, timeout: 5) else {
      let frame = windows.firstMatch.frame
      XCTFail("Workspace \(destination) is not clickable. Window frame: \(frame)")
      return
    }
    row.click()
  }
}
