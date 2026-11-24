import Foundation
import XCTest

extension XCTestCase {
  @MainActor
  func captureWindow(_ name: String, in app: XCUIApplication) {
    guard ProcessInfo.processInfo.environment["CTXFAB_CAPTURE_UI"] != "0" else { return }

    let popover = app.popovers.firstMatch
    let alert = app.alerts.firstMatch
    let window = app.windows["dashboard"]
    let target: XCUIElement
    if popover.exists {
      target = popover
    } else if alert.exists {
      target = alert
    } else if let sheet = app.sheets.allElementsBoundByIndex.last {
      target = sheet
    } else {
      target = window.exists ? window : app
    }

    let attachment = XCTAttachment(screenshot: target.screenshot())
    attachment.name = name
    attachment.lifetime = .keepAlways
    add(attachment)
  }
}
