import 'package:flutter/material.dart';
import 'package:flutter_native_splash/flutter_native_splash.dart';
import 'package:get_10101/backend.dart';
import 'package:get_10101/features/stable/stable_screen.dart';
import 'package:get_10101/features/trade/trade_screen.dart';
import 'package:get_10101/features/wallet/wallet_screen.dart';
import 'package:get_10101/features/welcome/new_wallet_screen.dart';
import 'package:get_10101/features/welcome/welcome_screen.dart';
import 'package:get_10101/logger/logger.dart';
import 'package:get_10101/util/preferences.dart';
import 'package:get_10101/util/file.dart';
import 'package:go_router/go_router.dart';

class LoadingScreen extends StatefulWidget {
  static const route = "/loading";

  const LoadingScreen({super.key});

  @override
  State<LoadingScreen> createState() => _LoadingScreenState();
}

class _LoadingScreenState extends State<LoadingScreen> {
  @override
  void initState() {
    super.initState();

    Future.wait<dynamic>([
      Preferences.instance.hasEmailAddress(),
      Preferences.instance.getOpenPosition(),
      isSeedFilePresent(),
    ]).then((value) {
      final hasEmailAddress = value[0];
      final position = value[1];
      final isSeedFilePresent = value[2];

      logger.d("Scanning for seed file: $isSeedFilePresent");

      if (isSeedFilePresent) {
        runBackend(context).then((value) {
          logger.i("Backend started");

          if (!hasEmailAddress) {
            GoRouter.of(context).go(WelcomeScreen.route);
          } else {
            switch (position) {
              case StableScreen.label:
                GoRouter.of(context).go(StableScreen.route);
              case TradeScreen.label:
                GoRouter.of(context).go(TradeScreen.route);
              default:
                GoRouter.of(context).go(WalletScreen.route);
            }
          }
        }).catchError((error) {
          logger.e("Failed to start backend. $error");
        }).whenComplete(() => FlutterNativeSplash.remove());
      } else {
        FlutterNativeSplash.remove();
        // No seed file: let the user choose whether they want to create a new
        // wallet or import their old one
        GoRouter.of(context).go(NewWalletScreen.route);
      }
    });
  }

  @override
  Widget build(BuildContext context) {
    return const Center(child: CircularProgressIndicator());
  }
}
