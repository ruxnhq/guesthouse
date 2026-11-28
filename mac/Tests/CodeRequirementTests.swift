// SPDX-License-Identifier: GPL-2.0-or-later
import Foundation
import Testing

struct CodeRequirementTests {
    @Test func aSignedPeerIsHeldToTheSameTeam() {
        #expect(
            codeRequirement(identifier: "com.ruxn.guesthouse", team: "AB12CD34EF")
                == #"identifier "com.ruxn.guesthouse" and anchor apple generic and certificate leaf[subject.OU] = "AB12CD34EF""#
        )
    }

    @Test func anAdHocBuildHoldsItsPeerToTheIdentifier() {
        #expect(codeRequirement(identifier: "com.ruxn.guesthouse.agent", team: nil) == #"identifier "com.ruxn.guesthouse.agent""#)
        #expect(codeRequirement(identifier: "com.ruxn.guesthouse.agent", team: "") == #"identifier "com.ruxn.guesthouse.agent""#)
    }

    @Test func aTeamThatIsNotLettersAndDigitsIsNoTeam() {
        #expect(codeRequirement(identifier: "x", team: "AB12 CD34") == #"identifier "x""#)
        #expect(codeRequirement(identifier: "x", team: "ÄB12CD34EF") == #"identifier "x""#)
    }
}
