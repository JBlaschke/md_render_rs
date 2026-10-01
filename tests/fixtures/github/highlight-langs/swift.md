```swift
import Foundation

struct Point: Codable {
    var x: Double
    let y: Double
}

func greet(_ name: String, times: Int = 1) -> String {
    guard !name.isEmpty else { return "" }
    return "Hello \(name)"
}

class ViewModel: ObservableObject {
    @Published var items: [String] = []
    private let id = UUID()
}
```
