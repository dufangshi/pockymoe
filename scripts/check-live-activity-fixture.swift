// Compile alongside the client's actual ios/Shared/ActivityPolicy.swift; this
// deliberately does not duplicate the ContentState model being verified.
import Foundation

@main
struct CheckLiveActivityFixture {
    struct Envelope: Decodable {
        struct APS: Decodable {
            var timestamp: Double
            var event: String
            var content: ActivityContentState
            var dismissalDate: Double?
            enum CodingKeys: String, CodingKey {
                case timestamp, event
                case content = "content-state"
                case dismissalDate = "dismissal-date"
            }
        }
        var aps: APS
    }
    static func main() throws {
        for path in CommandLine.arguments.dropFirst() {
            // ActivityKit requires default Codable strategies, including Date's
            // reference epoch. APNs envelope fields are independently Unix time.
            let envelope = try JSONDecoder().decode(Envelope.self, from: Data(contentsOf: URL(fileURLWithPath: path)))
            let content = envelope.aps.content
            precondition(content.totalCount == 3)
            precondition(content.rows.count == 3)
            precondition(content.earliestStart.timeIntervalSince1970 == 1_793_620_500)
            if envelope.aps.event == "end" {
                precondition(content.endedAt?.timeIntervalSince1970 == envelope.aps.timestamp)
                precondition(envelope.aps.dismissalDate == content.dismissalDate?.timeIntervalSince1970)
                precondition(content.completedCount == 3)
            } else {
                precondition(content.endedAt == nil)
                precondition(content.waitingCount == 1 && content.failedCount == 1 && content.runningCount == 1)
                precondition(content.rows[0].startedAt.timeIntervalSince1970 == envelope.aps.timestamp - 60)
            }
        }
        print("PASS: actual iOS ActivityContentState decoded both Relay fixtures with default Date strategies")
    }
}
