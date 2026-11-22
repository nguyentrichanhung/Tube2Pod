import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:mobile/platform/home_transport.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  test('request passes endpoint identity and credential to native transport', () async {
    const channel = MethodChannel('tube2pod/home_transport');
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger.setMockMethodCallHandler(channel, (call) async {
      expect(call.method, 'request');
      final args = call.arguments as Map;
      expect(args['serverKey'], 'spki');
      expect(args['serverId'], 'home');
      expect(args['headers'], {'Authorization': 'credential'});
      return {'status': 200, 'body': Uint8List.fromList([1])};
    });
    final response = await HomeTransport().request(
      HomeEndpoint(uri: Uri.parse('https://home/'), serverId: 'home', serverKey: 'spki'),
      const HomeRequest(headers: {'Authorization': 'credential'}),
    );
    expect(response.status, 200);
    expect(response.body, [1]);
  });
}
