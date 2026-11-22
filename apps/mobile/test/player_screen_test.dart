// ignore_for_file: deprecated_member_use
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:mobile/features/listen/listen_screen.dart';
import 'package:mobile/platform/home_transport.dart';

void main() {
  testWidgets('player shows position and toggles the existing session', (
    tester,
  ) async {
    var playing = true;
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
          const MethodChannel('tube2pod/home_transport'),
          (call) async {
            if (call.method == 'position') {
              return {
                'position': 10000,
                'duration': 120000,
                'playing': playing,
              };
            }
            return null;
          },
        );
    await tester.pumpWidget(
      MaterialApp(
        home: PlayerScreen(
          episode: const {'title': 'Episode', 'show_title': 'Show'},
          player: HomePlayer(),
          cover: const Icon(Icons.album),
          toggle: () async {
            playing = !playing;
          },
        ),
      ),
    );
    await tester.pump();
    expect(find.text('Episode'), findsOneWidget);
    expect(find.text('0:10'), findsOneWidget);
    expect(find.text('2:00'), findsOneWidget);
    await tester.tap(find.text('Tạm dừng'));
    await tester.pump();
    expect(find.text('Nghe tập'), findsOneWidget);
    await tester.pumpWidget(const SizedBox.shrink());
  });
}
