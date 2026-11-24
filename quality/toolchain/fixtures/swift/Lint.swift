func lintFixture(_ value: Any) -> Int {
  value as! Int
}

func unwrapFixture(_ value: Int?) -> Int {
  value!
}

func throwingTaskFixture(_ operation: @escaping @Sendable () throws -> Void) {
  Task {
    try operation()
  }
}
