// SPDX-License-Identifier: GPL-2.0-or-later
import Foundation
import Testing

struct MachineStatusTests {
    @Test func stoppedMachineSaysOff() {
        let status = MachineStatus.off
        #expect(status.sentence(for: "Windows 11") == "Windows 11 is turned off.")
    }

    @Test func runningSaysItIsRunning() {
        let status = MachineStatus(state: "running", reason: nil)
        #expect(status.sentence(for: "W").contains("is running"))
    }

    @Test func aShutdownThatStalledWarnsOfForceOff() {
        let stalled = MachineStatus(state: "stopping_stalled", reason: nil)
        #expect(stalled.sentence(for: "W").contains("Force Off"))
    }

    @Test func aFailureGivesItsReason() {
        let status = MachineStatus(state: "failed", reason: "the disk image is not there")
        #expect(status.sentence(for: "W").hasSuffix("the disk image is not there"))
        #expect(MachineStatus(state: "failed", reason: nil).sentence(for: "W").hasSuffix("went wrong."))
    }
}

struct MachineFormatTests {
    @Test func modesReadAsSizeAndRate() {
        #expect(MachineFormat.mode("1024x768@60") == "1024 × 768, 60 Hz")
        #expect(MachineFormat.mode("odd") == "odd")
        #expect(MachineFormat.mode("axb@c") == "axb@c")
    }

    @Test func aModeSplitsIntoSizeAndRate() {
        let parts = MachineFormat.modeParts("1024x768@60")
        #expect(parts?.size == "1024 × 768" && parts?.rate == "60 Hz")
        #expect(MachineFormat.modeParts("odd") == nil)
        #expect(MachineFormat.systemName("windows") == "Windows")
    }

    @Test func memoryReadsInGigabytes() {
        #expect(MachineFormat.memory(megabytes: 6144).contains("6"))
    }

    @Test func diskNamesBothSizesOrSaysItIsGone() {
        #expect(MachineFormat.disk(size: nil, onDisk: nil) == "The disk image is not there")
        #expect(MachineFormat.disk(size: 64_000_000_000, onDisk: 20_000_000_000).contains("used on this Mac"))
    }

    @Test func graphicsIsNamedInPlainWords() {
        #expect(MachineFormat.graphicsTitle("guarded") == "Protected graphics (recommended)")
        #expect(MachineFormat.graphicsTitle("qemu") == "QEMU's own device")
        // A file from before the setting is the protected path, which the core says as "guarded".
        #expect(MachineFormat.graphicsTitle("anything else") == "Protected graphics (recommended)")
    }

    @Test func windowsIsDescribed() {
        #expect(MachineFormat.kindDescription("windows") == "Windows 11 on ARM")
    }
}

struct DiskUsageTests {
    @Test func aSparseFileTakesLessThanItsSize() throws {
        let path = NSTemporaryDirectory() + "gh-disk-\(ProcessInfo.processInfo.processIdentifier).img"
        defer { try? FileManager.default.removeItem(atPath: path) }
        FileManager.default.createFile(atPath: path, contents: nil)
        let handle = try #require(FileHandle(forWritingAtPath: path))
        try handle.truncate(atOffset: 1_000_000_000)
        try handle.close()
        let usage = try #require(DiskUsage.read(path: path))
        #expect(usage.size == 1_000_000_000)
        #expect(usage.onDisk < usage.size)
    }

    @Test func aMissingFileReadsAsNil() {
        #expect(DiskUsage.read(path: "/no/such/disk.img") == nil)
    }
}

struct ShareStatusTests {
    @Test func offAndWaitingAreSaidPlainly() {
        #expect(ShareStatus.headline(allowed: false, machineRunning: true, share: .off).hasPrefix("Off."))
        #expect(ShareStatus.headline(allowed: true, machineRunning: false, share: .off).contains("once the machine is running"))
    }

    @Test func listeningNamesTheViewerOrWaits() {
        let ready = ShareStatus(json: ["state": "listening", "port": 7792, "fingerprint": "ab12", "pairing": false])!
        #expect(ShareStatus.headline(allowed: true, machineRunning: true, share: ready).hasPrefix("Ready"))
        let busy = ShareStatus(json: ["state": "listening", "viewer": "Pat's iPad"])!
        #expect(ShareStatus.headline(allowed: true, machineRunning: true, share: busy) == "Pat's iPad is viewing this machine.")
    }

    @Test func aFailureGivesItsReason() {
        let failed = ShareStatus(json: ["state": "failed", "reason": "rmhost exited with status 1"])!
        #expect(ShareStatus.headline(allowed: true, machineRunning: true, share: failed).hasSuffix("rmhost exited with status 1"))
    }

    @Test func theAgentsEventIsRead() throws {
        let status = try #require(ShareStatus(json: [
            "state": "listening", "port": 7792, "fingerprint": "ab12", "pairing": true, "pairing_until_ms": 1_000_000.0,
        ]))
        #expect(status.port == 7792 && status.pairing)
        #expect(status.pairingUntil == Date(timeIntervalSince1970: 1000))
        #expect(ShareStatus(json: ["state": "nonsense"]) == nil)
    }
}

struct LocalNetworkTests {
    @Test func theAddressShownIsOneAnotherDeviceCanReach() {
        let all = [
            LocalNetwork.Candidate(interface: "lo0", address: "127.0.0.1"),
            LocalNetwork.Candidate(interface: "en1", address: "169.254.3.4"),
            LocalNetwork.Candidate(interface: "bridge100", address: "192.168.64.1"),
            LocalNetwork.Candidate(interface: "en0", address: "192.168.1.20"),
        ]
        #expect(LocalNetwork.best(from: all) == "192.168.1.20")
        #expect(LocalNetwork.best(from: [all[0], all[1]]) == nil)
    }
}

struct ScreenStatusTests {
    private func line(screen: Bool = true, alive: Bool, picture: ScreenStatus.Picture?) -> String {
        ScreenStatus.line(hasScreen: screen, startable: true, machineAlive: alive, picture: picture)
    }

    @Test func aStillPictureIsNotSaidToBeZeroFramesASecond() {
        #expect(ScreenStatus.rateText(framesPerSecond: 0) == "The picture is still")
        #expect(ScreenStatus.rateText(framesPerSecond: 10) == "10 frames a second")
    }

    @Test func theLineFollowsTheMachineAndTheWindow() {
        #expect(line(screen: false, alive: false, picture: nil).contains("no screen to show"))
        #expect(line(alive: false, picture: nil).contains("no screen while the machine is turned off"))
        #expect(line(alive: true, picture: nil).contains("Open Screen"))
        #expect(line(alive: true, picture: .waiting).contains("first picture"))
        let showing = ScreenStatus.Picture.showing(width: 1024, height: 768, framesPerSecond: 10)
        #expect(line(alive: true, picture: showing) == "Showing 1024 × 768, 10 frames a second.")
    }
}

struct PairingTextTests {
    private func share(_ json: [String: Any]) -> ShareStatus {
        var all: [String: Any] = ["state": "listening"]
        all.merge(json) { _, new in new }
        return ShareStatus(json: all)!
    }

    @Test func openPairingCountsDown() {
        let now = Date(timeIntervalSince1970: 1000)
        let open = share(["pairing": true, "pairing_until_ms": 1_090_000.0])
        #expect(open.secondsLeft(now: now) == 90)
        #expect(open.secondsLeft(now: Date(timeIntervalSince1970: 1100)) == nil)
        #expect(share(["pairing": false]).secondsLeft(now: now) == nil)
    }

    @Test func closedSaysHowItEnded() {
        #expect(share(["closed_reason": "paired", "paired": "Pat's iPad"]).closedText == "Closed. Pat's iPad was paired.")
        #expect(share(["closed_reason": "timeout"]).closedText == "Closed. Nobody paired in time.")
        #expect(share(["closed_reason": "declined"]).closedText.contains("Press Pair a New Device to try again"))
        #expect(share(["closed_reason": "asked"]).closedText == "Closed")
        #expect(share([:]).closedText == "Closed")
    }

    @Test func aWaitingDeviceHasItsNameAndCode() throws {
        let waiting = share(["pairing": false, "pending": ["name": "Pat's iPad", "fingerprint": "ab", "code": "123 456", "seconds": 60.0]])
        let device = try #require(waiting.pending)
        #expect(device.name == "Pat's iPad" && device.code == "123 456")
        #expect(device.until > Date().addingTimeInterval(55))
        #expect(!waiting.pairing)
    }

    @Test func theStepsAskForTheCodeNotNobody() {
        #expect(ShareStatus.pairingSteps.joined().contains("code"))
        #expect(!ShareStatus.pairingSteps.joined().contains("Nobody is asked"))
        #expect(ShareStatus.compareCodeLine.contains("matches the one shown on the device"))
    }
}

struct InstallStatusTests {
    private func status(_ json: [String: Any]) -> InstallStatus {
        var all: [String: Any] = ["machine": "ubuntu", "guest": "ubuntu", "state": "running", "steps": [
            ["step": "fetch", "status": "done", "progress": ["fraction": 1.0]],
            ["step": "install_system", "status": "running", "progress": ["fraction": 0.38]],
        ]]
        all.merge(json) { _, new in new }
        return InstallStatus(json: all)!
    }

    @Test func aRunningInstallPrintsItsStepItsPercentAndTheTimeLeft() {
        let line = status(["current": "install_system", "seconds_left": 840]).printLine()
        #expect(line == "install: ubuntu ubuntu running install_system 38% About 14 minutes left.")
    }

    @Test func aFailurePrintsWhereItStoppedItsCodeItsReasonAndItsChoices() {
        let failure: [String: Any] = ["step": "install_driver", "code": "0x800b0109", "reason": "Windows turned the driver away.", "choices": ["try_again", "open_without_driver"]]
        let line = status(["state": "failed", "failure": failure]).printLine()
        #expect(line.contains("stopped at install_driver (0x800b0109): Windows turned the driver away."))
        #expect(line.hasSuffix("choices: try_again, open_without_driver"))
    }

    @Test func timeLeftIsSaidInMinutesAsTheScreenDoes() {
        #expect(InstallStatus.timeLeft(seconds: 20) == "Less than a minute left.")
        #expect(InstallStatus.timeLeft(seconds: 60) == "About a minute left.")
        #expect(InstallStatus.timeLeft(seconds: 14 * 60) == "About 14 minutes left.")
    }

    @Test func somethingThatIsNotAnInstallIsNotRead() {
        #expect(InstallStatus(json: ["machine": "x"]) == nil)
    }
}

struct GraphicsGuardTests {
    @Test func aRefusalIsSaidInPlainWordsAndQuietIsNot() {
        #expect(GraphicsCounts(published: 5, refused: 0, spoiled: false).warning(system: "Windows") == nil)
        #expect(GraphicsCounts(published: 5, refused: 1, spoiled: false).warning(system: "Windows")
            == "The graphics guard refused a command from Windows. The screen may be wrong.")
        #expect(GraphicsCounts(published: 5, refused: 0, spoiled: true).warning(system: "Windows") != nil)
    }
}

struct DeviceListTests {
    @Test func theDevicesAreRead() throws {
        let status = try #require(ShareStatus(json: [
            "state": "listening",
            "devices": [
                ["fingerprint": "ab12", "name": "Pat's iPad", "connected": true,
                 "permissions": ["input": true, "clipboard": false]],
                ["fingerprint": "cd34", "name": "Old phone", "connected": false,
                 "permissions": ["input": false]],
            ],
        ]))
        #expect(status.devices.count == 2)
        #expect(status.devices[0].statusText == "Viewing now" && status.devices[0].control)
        #expect(status.devices[1].statusText == "Not connected" && !status.devices[1].control)
    }

    @Test func aShareStateIsPrintedOnOneLine() throws {
        let share = try #require(ShareStatus(json: [
            "state": "listening", "port": 7792, "pairing": false,
            "pending": ["name": "Pat's iPad", "fingerprint": "ab", "code": "123 456", "seconds": 60.0],
            "viewer": "Pat's iPad",
        ]))
        #expect(share.printLine(machine: "win") == "share: win listening port 7792 pairing closed pending Pat's iPad code 123 456 viewer Pat's iPad")
        #expect(ShareStatus(json: ["state": "failed", "reason": "no rmhost"])?.printLine(machine: "w") == "share: w failed pairing closed reason: no rmhost")
    }

    @Test func theFingerprintIsShortenedInGroups() throws {
        let long = try #require(ShareStatus(json: ["state": "listening", "fingerprint": "0123456789abcdef0123"]))
        #expect(long.shortFingerprint == "0123 4567 89ab cdef…")
        let short = try #require(ShareStatus(json: ["state": "listening", "fingerprint": "0123"]))
        #expect(short.shortFingerprint == "0123")
        #expect(ShareStatus(json: ["state": "listening"])?.shortFingerprint == nil)
    }
}

struct NewMachineLogicTests {
    @Test func firmwareIsToldApartByName() {
        let found = FirmwareFiles.sorted(["/m/vars.fd", "/m/code.fd", "/m/pvgfb.rom", "/m/win.img", "/m/notes.txt"])
        #expect(found.code == "/m/code.fd" && found.vars == "/m/vars.fd" && found.optionROM == "/m/pvgfb.rom")
        #expect(found.complete)
        #expect(!FirmwareFiles.sorted(["/m/code.fd"]).complete)
    }

    @Test func firmwareIsFoundBesideTheDisk() throws {
        let folder = NSTemporaryDirectory() + "gh-fw-\(ProcessInfo.processInfo.processIdentifier)/"
        try FileManager.default.createDirectory(atPath: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(atPath: folder) }
        for name in ["win.img", "code.fd", "vars.fd"] {
            FileManager.default.createFile(atPath: folder + name, contents: Data())
        }
        let found = FirmwareFiles.beside(disk: folder + "win.img")
        #expect(found.complete && found.optionROM == nil)
        #expect(!FirmwareFiles.beside(disk: "").complete)
    }
}

struct InstallTextTests {
    private func status(guest: String = "windows", failure: [String: Any]? = nil) -> InstallStatus {
        var json: [String: Any] = [
            "machine": "w", "guest": guest, "state": failure == nil ? "running" : "failed", "current": "install_system",
            "steps": [
                ["step": "fetch", "status": "done"],
                ["step": "make_disk", "status": "done"],
                ["step": "install_system", "status": "running", "progress": ["fraction": 0.38]],
                ["step": "install_driver", "status": "pending"],
                ["step": "first_start", "status": "pending"],
            ],
        ]
        if let failure { json["failure"] = failure }
        return InstallStatus(json: json)!
    }

    @Test func theScreensListFourStepsAndLeaveTheDiskOut() {
        #expect(status().shown.map(\.name) == ["fetch", "install_system", "install_driver", "first_start"])
        #expect(status().title(of: "fetch") == "Download Windows")
        #expect(status(guest: "ubuntu").title(of: "install_system") == "Install Ubuntu")
        #expect(status().percent == 38)
        #expect(status().sidebarLine == "Installing, 38%")
    }

    @Test func aStoppedDriverSaysNothingIsLostAndOffersTheCoresChoices() {
        let failure: [String: Any] = ["step": "install_driver", "code": "0x800b0109", "reason": "Windows turned the driver away.", "choices": ["try_again", "open_without_driver", "send_report"]]
        let stopped = status(failure: failure)
        #expect(stopped.failureTitle == "The graphics driver didn't install")
        #expect(stopped.failureSentence == "Windows is installed and nothing is lost. Windows turned the driver away.")
        #expect(stopped.label(forChoice: "open_without_driver") == "Open Windows Without It")
        #expect(stopped.label(forChoice: "try_again") == "Try Again")
    }

    @Test func sizesFollowTheMacAndTheSpaceIsCountedLikeTheCore() {
        #expect(InstallGuest.windows.cores(hostCores: 10) == 4)
        #expect(InstallGuest.windows.cores(hostCores: 2) == 2)
        #expect(InstallGuest.windows.spaceNeededGB(diskGB: 64) == 36)
        #expect(InstallGuest.ubuntu.spaceNeededGB(diskGB: 32) == 20)
        let roomy = InstallText.spaceSentence(needGB: 36, freeGB: 212, diskGB: 64)
        #expect(roomy.text == "Needs 36 GB. This Mac has 212 GB free." && roomy.enough && roomy.warning == nil)
        #expect(InstallText.spaceSentence(needGB: 36, freeGB: 20).enough == false)
        // 24 GB free is enough for an Ubuntu install of a 32 GB disk, and says the disk could outgrow it.
        let tight = InstallText.spaceSentence(needGB: 20, freeGB: 24, diskGB: 32)
        #expect(tight.enough)
        #expect(tight.warning?.contains("can grow by up to 32 GB and this Mac has 24 GB free") == true)
    }

    @Test func aTakenNameGetsTheNextNumber() {
        #expect(InstallText.freeName("Ubuntu", taken: []) == "Ubuntu")
        #expect(InstallText.freeName("Ubuntu", taken: ["Ubuntu", "Ubuntu 2"]) == "Ubuntu 3")
    }
}

struct GuestPasswordTests {
    @Test func aPasswordIsFourShortGroupsWithNothingThatLooksAlike() {
        let password = GuestPassword.generate()
        let groups = password.split(separator: "-")
        #expect(groups.count == 4 && groups.allSatisfy { $0.count == 4 })
        #expect(password.allSatisfy { $0 == "-" || GuestPassword.alphabet.contains($0) })
        for ambiguous in "01oOlIi" { #expect(!password.contains(ambiguous)) }
    }

    @Test func twoPasswordsDiffer() {
        #expect(GuestPassword.generate() != GuestPassword.generate())
    }

    @Test func theCoreTakesWhatIsMade() {
        // The core accepts letters, digits and dashes, 8 to 64 of them.
        let password = GuestPassword.generate()
        #expect((8...64).contains(password.count))
    }
}

struct ScreenSizingTests {
    @Test func sharpAsksForTwiceThePointsAndTwoHundredPercent() {
        let wanted = WantedScreen.wanted(points: CGSize(width: 1180, height: 696), sharp: true)
        #expect(wanted == WantedScreen(pointsWidth: 1180, pointsHeight: 696, pixelsWidth: 2360, pixelsHeight: 1392, scalingPercent: 200))
    }

    @Test func standardAsksForThePointsAsPixels() {
        let wanted = WantedScreen.wanted(points: CGSize(width: 1180, height: 696), sharp: false)
        #expect(wanted.pixelsWidth == 1180 && wanted.pixelsHeight == 696 && wanted.scalingPercent == 100)
    }

    @Test func oddSizesAreMadeEvenAndTinyOnesRaisedToTheLeast() {
        let odd = WantedScreen.wanted(points: CGSize(width: 1181, height: 697), sharp: false)
        #expect(odd.pixelsWidth == 1180 && odd.pixelsHeight == 696)
        let tiny = WantedScreen.wanted(points: CGSize(width: 100, height: 100), sharp: false)
        #expect(tiny.pixelsWidth == 640 && tiny.pixelsHeight == 400)
    }

    @Test func aWindowIsNeverAskedForMoreThanEightK() {
        let big = WantedScreen.wanted(points: CGSize(width: 9000, height: 5000), sharp: true)
        #expect(big.pixelsWidth == 7680 && big.pixelsHeight == 4320)
    }
}

struct ProblemTextTests {


    @Test func aMachineThatDidNotStartSaysNothingWasChanged() {
        let problem = Problem.didNotStart(machine: "Windows 11", machineID: "x")
        #expect(problem.title == "Windows 11 didn't start")
        #expect(problem.message.contains("Nothing on its disk was changed"))
    }
}

struct ReportTextTests {
    @Test func theKindsAreTheLinuxAppsWordsAndLabels() {
        #expect(ReportKind.allCases.map(\.rawValue) == ["wont-start", "crashed", "picture", "slow", "install-failed", "other"])
        #expect(ReportKind.allCases.map(\.label) == ["Won't start", "Crashed", "Picture is wrong", "Slow", "Install failed", "Something else"])
    }

    @Test func whatIsSentSaysWhereItGoesHowLongAndThatNothingGoesUntilSend() {
        let text = ReportWords.whatIsSent
        #expect(text.contains("deleted after 30 days"))
        #expect(text.contains("Nothing is sent until you press Send."))
        #expect(text.contains("passwords, pairing keys, device fingerprints"))
    }
}

struct UpdateTextTests {
    @Test func theCoresAnswerIsRead() {
        #expect(UpdateState(json: #"{"state":"newer","version":"0.2.0","where":"https://x/y"}"#) == .newer(version: "0.2.0", page: "https://x/y"))
        #expect(UpdateState(json: #"{"state":"latest"}"#) == .latest)
        #expect(UpdateState(json: "{}") == .cantTell)
        #expect(UpdateState(json: "garbled") == .cantTell)
    }

    @Test func theWordsSayWhatIsOutAndThatNothingIsDownloaded() {
        let newer = UpdateState.newer(version: "0.2.0", page: "p")
        #expect(newer.headline(current: "0.1.0") == "Guesthouse 0.2.0 is out")
        #expect(newer.detail(lastChecked: nil).contains("does not download or install it by itself"))
        #expect(UpdateState.latest.headline(current: "0.1.0") == "Guesthouse 0.1.0 is up to date")
    }
}

struct StartCheckTests {


    @Test func theDriverMessageIsShownOnlyWhenOldOrMissing() {
        let old = Problem.driver(json: #"{"state":"old","reported":"0.1.0","released":"0.2.0"}"#, machine: "W", machineID: "w")
        #expect(old?.title == "The graphics driver in W is older than this Guesthouse")
        let missing = Problem.driver(json: #"{"state":"missing","released":"0.2.0"}"#, machine: "W", machineID: "w")
        #expect(missing?.title == "The graphics driver in W is missing")
        #expect(Problem.driver(json: #"{"state":"unknown"}"#, machine: "W", machineID: "w") == nil)
        #expect(Problem.driver(json: #"{"state":"current"}"#, machine: "W", machineID: "w") == nil)
        #expect(Problem.driver(json: "garbled", machine: "W", machineID: "w") == nil)
    }
}

struct InstallRepairTests {
    private func status(code: String?, state: String = "failed") -> InstallStatus {
        var json: [String: Any] = ["machine": "w", "guest": "windows", "state": state, "steps": []]
        if let code { json["failure"] = ["step": "install_system", "code": code, "reason": "r", "choices": ["try_again"]] }
        return InstallStatus(json: json)!
    }

    @Test func onlyAMissingPasswordIsRepairedByTheApp() {
        #expect(status(code: "password_missing").needsPassword)
        #expect(!status(code: "no_space").needsPassword)
        #expect(!status(code: nil, state: "running").needsPassword)
    }
}

struct ProblemFromCoreTests {
    @Test func theCoresMemoryAnswerBecomesAMessageWithItsButtons() {
        let answer = #"{"problem":{"kind":"not_enough_memory","machine":"Ubuntu","title":"Not enough memory to start Ubuntu","message":"m","running":"Windows 11","start_anyway":false}}"#
        let problem = Problem.from(core: answer)
        #expect(problem == .notEnoughMemory(machine: "Ubuntu", title: "Not enough memory to start Ubuntu", message: "m", running: "Windows 11", startAnyway: false))
        #expect(problem?.title == "Not enough memory to start Ubuntu")
    }

    @Test func aDiskAnswerAndNoProblemAreRead() {
        let disk = #"{"problem":{"kind":"not_enough_disk","machine":"W","title":"t","message":"m","start_anyway":true}}"#
        #expect(Problem.from(core: disk) == .notEnoughDisk(machine: "W", title: "t", message: "m", startAnyway: true))
        #expect(Problem.from(core: #"{"problem":null}"#) == nil)
        #expect(Problem.from(core: "{}") == nil)
        #expect(Problem.from(core: "garbled") == nil)
    }

    @Test func theCoresVerdictOnAnotherAgentIsRead() {
        #expect(AgentVerdict.from(core: #"{"verdict":"own"}"#) == .own)
        #expect(AgentVerdict.from(core: #"{"verdict":"replace","older":"0.1.1 (1)"}"#) == .replace(older: "0.1.1 (1)"))
        #expect(AgentVerdict.from(core: #"{"verdict":"busy","older":"an older build","running":["a","b"],"installing":[],"can_continue":true}"#)
            == .busy(older: "an older build", running: ["a", "b"], installing: [], canContinue: true))
        #expect(AgentVerdict.from(core: #"{"error":"x"}"#) == nil)
    }

    @Test func theOlderAgentSheetNamesTheMachinesAndWhatEachChoiceDoes() {
        let problem = Problem.olderAgent(older: "0.1.1 (1)", machines: ["Windows 11", "Ubuntu"], installing: [], canContinue: true)
        #expect(problem.title == "An older Guesthouse is still running machines")
        #expect(problem.message.contains("Guesthouse 0.1.1 (1) is still running Windows 11 and Ubuntu."))
        #expect(problem.message.contains("works with the older one until you quit"))
        let stuck = Problem.olderAgent(older: "0.1.1 (1)", machines: ["Windows 11"], installing: ["Windows 11"], canContinue: false)
        #expect(stuck.message.contains("Windows 11 is being installed"))
        #expect(stuck.message.contains("shows no machines until they are shut down"))
    }

    @Test func launchdIsGivenUpAfterTwoAgentsOfAnotherCopyInARow() {
        var patience = LaunchdPatience()
        #expect(patience.replacedOne() == false)
        #expect(patience.replacedOne() == true)
    }
}
