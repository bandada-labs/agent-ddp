import XCTest
@testable import AgentDDP

final class DrawingTests: XCTestCase {
    // What RenderBox wrote for the sample's Canvas on iOS 26.5, trimmed.
    private let canvas = """
    <?xml version="1.0" encoding="UTF-8"?>
    <display-list xmlns="http://www.apple.com/RenderBox/1.0">
      <transform translation="-16 -0"/>
      <display-list id="@1">
        <device scale="3"/>
        <draw circle="60 30" radius="20" color="rgb(0, 0, 1)"/>
        <draw stroked-circle="60 30" radius="20" line-width="4" color="rgb(0, 0, 0)"/>
        <copy-state to="#1"/>
        <draw state="#1" rect="2 2 20 8" color="rgb(0.203979, 0.780273, 0.349121)" headroom="1" alpha="0.5"/>
        <transform state="#1" translation="30 0"/>
        <draw state="#1" corner-style="continuous" corner-radius="3" rect="2 2 20 8" color="rgba(0, 0.533203, 1, 0.5)" alpha="0.5"/>
        <draw stroked-path="0 58 m 80 50 l" line-width="2" color="rgb(1, 0.552734, 0.15686)" headroom="1"/>
        <draw path="60 10 m 80 0 78 20 q h" color="rgb(1.08984, -0.0823975, 0.123413)"/>
        <draw ellipse="40 40 20 10" start-point="0 0" end-point="80 60" axial-gradient="rgb(1, 0, 0) rgb(0, 0, 1)"/>
        <draw rect="0 0 4 4" color="rgba(1, 0, 0, 0)"/>
        <layer><transform-style><color from="rgba(-1, -1, -1, 1)"/></transform-style><draw rect="0 0 1 1" color="rgb(0, 0, 0)"/></layer>
      </display-list>
    </display-list>
    """

    func testEachDrawIsReadInTheLayersCoordinates() throws {
        let read = try XCTUnwrap(DisplayListReader.read(canvas))
        XCTAssertEqual(read.draws.map(\.op), ["circle", "circle", "rect", "roundRect", "line", "path", "oval"])
        XCTAssertEqual(read.draws[0].bounds, CGRect(x: 24, y: 10, width: 40, height: 40))
        XCTAssertEqual(read.draws[0].fill, "#0000FFFF")
        XCTAssertEqual(read.draws[1].border?.width, 4)
        // The copied state moves only the draws that use it; opacity and color alpha multiply.
        XCTAssertEqual(read.draws[2].bounds, CGRect(x: -14, y: 2, width: 20, height: 8))
        XCTAssertEqual(read.draws[2].fill, "#34C75980")
        XCTAssertEqual(read.draws[3].bounds, CGRect(x: 16, y: 2, width: 20, height: 8))
        XCTAssertEqual(read.draws[3].fill, "#0088FF40")
        XCTAssertEqual(read.draws[4].bounds, CGRect(x: -16, y: 50, width: 80, height: 8))
        // The curve's own box, not its control point's (which reaches y 0 and x 80).
        XCTAssertEqual(read.draws[5].bounds.minY, 10 - 10 / 3, accuracy: 0.01)
        XCTAssertEqual(read.draws[5].bounds.maxX, 78.18 - 16, accuracy: 0.01)
        XCTAssertEqual(read.draws[5].fill, "#FF001FFF")
        XCTAssertTrue(read.draws[6].shader)
        // The transparent rect draws nothing; the text layer is drawn but not read.
        XCTAssertEqual(read.unread, 1)
    }

    func testAClipOrATransformItDoesNotKnowMakesTheListUnknown() {
        let clipped = canvas.replacingOccurrences(of: "<device scale=\"3\"/>", with: "<clip-shape rect=\"0 0 10 10\"/>")
        XCTAssertNil(DisplayListReader.read(clipped))
        let rotated = canvas.replacingOccurrences(of: "translation=\"30 0\"", with: "rotation=\"0.5\"")
        XCTAssertNil(DisplayListReader.read(rotated))
    }
}
