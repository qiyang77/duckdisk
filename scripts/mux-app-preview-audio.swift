import AVFoundation
import Foundation

guard CommandLine.arguments.count == 4 else {
    fputs("usage: mux-app-preview-audio.swift VIDEO AUDIO OUTPUT\n", stderr)
    exit(2)
}

let videoURL = URL(fileURLWithPath: CommandLine.arguments[1])
let audioURL = URL(fileURLWithPath: CommandLine.arguments[2])
let outputURL = URL(fileURLWithPath: CommandLine.arguments[3])

try? FileManager.default.removeItem(at: outputURL)

let videoAsset = AVURLAsset(url: videoURL)
let audioAsset = AVURLAsset(url: audioURL)
let composition = AVMutableComposition()

guard
    let sourceVideo = videoAsset.tracks(withMediaType: .video).first,
    let destinationVideo = composition.addMutableTrack(
        withMediaType: .video,
        preferredTrackID: kCMPersistentTrackID_Invalid
    )
else {
    fputs("missing video track\n", stderr)
    exit(3)
}

let videoRange = CMTimeRange(start: .zero, duration: videoAsset.duration)
try destinationVideo.insertTimeRange(videoRange, of: sourceVideo, at: .zero)
destinationVideo.preferredTransform = sourceVideo.preferredTransform

guard
    let sourceAudio = audioAsset.tracks(withMediaType: .audio).first,
    let destinationAudio = composition.addMutableTrack(
        withMediaType: .audio,
        preferredTrackID: kCMPersistentTrackID_Invalid
    )
else {
    fputs("missing audio track\n", stderr)
    exit(4)
}

let audioDuration = CMTimeMinimum(audioAsset.duration, videoAsset.duration)
try destinationAudio.insertTimeRange(
    CMTimeRange(start: .zero, duration: audioDuration),
    of: sourceAudio,
    at: .zero
)

guard let exporter = AVAssetExportSession(
    asset: composition,
    presetName: AVAssetExportPresetPassthrough
) else {
    fputs("could not create exporter\n", stderr)
    exit(5)
}

exporter.outputURL = outputURL
exporter.outputFileType = .mov
exporter.shouldOptimizeForNetworkUse = true

let semaphore = DispatchSemaphore(value: 0)
exporter.exportAsynchronously {
    semaphore.signal()
}
semaphore.wait()

switch exporter.status {
case .completed:
    print(outputURL.path)
case .failed, .cancelled:
    fputs("export failed: \(exporter.error?.localizedDescription ?? "unknown error")\n", stderr)
    exit(6)
default:
    fputs("export ended with status \(exporter.status.rawValue)\n", stderr)
    exit(7)
}
