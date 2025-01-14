import 'package:flutter/material.dart';
import 'package:get_10101/common/balance.dart';
import 'package:get_10101/common/version_service.dart';
import 'package:get_10101/logger/logger.dart';
import 'package:get_10101/trade/orderbook_service.dart';
import 'package:get_10101/wallet/wallet_service.dart';
import 'package:go_router/go_router.dart';
import 'package:provider/provider.dart';

class NavigationDestinations {
  const NavigationDestinations(this.label, this.icon, this.selectedIcon);

  final String label;
  final Widget icon;
  final Widget selectedIcon;
}

const List<NavigationDestinations> destinations = <NavigationDestinations>[
  NavigationDestinations('Trading', Icon(Icons.bar_chart_outlined), Icon(Icons.bar_chart)),
  NavigationDestinations('Wallet', Icon(Icons.wallet_outlined), Icon(Icons.wallet)),
  NavigationDestinations('Settings', Icon(Icons.settings_outlined), Icon(Icons.settings)),
];

class ScaffoldWithNestedNavigation extends StatefulWidget {
  const ScaffoldWithNestedNavigation({
    Key? key,
    required this.navigationShell,
  }) : super(key: key ?? const ValueKey<String>('ScaffoldWithNestedNavigation'));
  final StatefulNavigationShell navigationShell;

  @override
  State<ScaffoldWithNestedNavigation> createState() => _ScaffoldWithNestedNavigation();
}

// Based on
// https://github.com/flutter/packages/blob/main/packages/go_router/example/lib/stateful_shell_route.dart
class _ScaffoldWithNestedNavigation extends State<ScaffoldWithNestedNavigation> {
  late bool showNavigationDrawer;
  late bool showAsDrawer;

  String version = "unknown";
  Balance balance = Balance.zero();
  BestQuote? bestQuote;

  void _goBranch(int index) {
    widget.navigationShell.goBranch(
      index,
      initialLocation: index == widget.navigationShell.currentIndex,
    );
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    showNavigationDrawer = MediaQuery.of(context).size.width >= 450;
    showAsDrawer = MediaQuery.of(context).size.width >= 1024;
  }

  @override
  void initState() {
    super.initState();
    context.read<VersionService>().fetchVersion().then((v) => setState(() => version = v.version));
    context.read<WalletService>().getBalance().then((b) => setState(() => balance = b));
    context.read<QuoteService>().fetchQuote().then((q) => setState(() {
          bestQuote = q;
        }));
  }

  @override
  Widget build(BuildContext context) {
    final navigationShell = widget.navigationShell;

    if (showNavigationDrawer) {
      return ScaffoldWithNavigationRail(
        body: navigationShell,
        selectedIndex: navigationShell.currentIndex,
        onDestinationSelected: _goBranch,
        showAsDrawer: showAsDrawer,
        version: version,
        balance: balance,
        bestQuote: bestQuote,
      );
    } else {
      return ScaffoldWithNavigationBar(
        body: navigationShell,
        selectedIndex: navigationShell.currentIndex,
        onDestinationSelected: _goBranch,
      );
    }
  }
}

class ScaffoldWithNavigationBar extends StatelessWidget {
  const ScaffoldWithNavigationBar({
    super.key,
    required this.body,
    required this.selectedIndex,
    required this.onDestinationSelected,
  });

  final Widget body;
  final int selectedIndex;
  final ValueChanged<int> onDestinationSelected;

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: body,
      bottomNavigationBar: NavigationBar(
        selectedIndex: selectedIndex,
        destinations: const [
          NavigationDestination(label: 'Trading', icon: Icon(Icons.bar_chart)),
          NavigationDestination(label: 'Wallet', icon: Icon(Icons.wallet)),
          NavigationDestination(label: 'Settings', icon: Icon(Icons.settings)),
        ],
        onDestinationSelected: onDestinationSelected,
      ),
    );
  }
}

class ScaffoldWithNavigationRail extends StatelessWidget {
  const ScaffoldWithNavigationRail({
    super.key,
    required this.body,
    required this.selectedIndex,
    required this.onDestinationSelected,
    required this.showAsDrawer,
    required this.version,
    required this.balance,
    required this.bestQuote,
  });

  final Widget body;
  final int selectedIndex;
  final ValueChanged<int> onDestinationSelected;
  final bool showAsDrawer;
  final String version;
  final Balance balance;
  final BestQuote? bestQuote;

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: Row(
        children: [
          NavigationRail(
            extended: showAsDrawer,
            selectedIndex: selectedIndex,
            onDestinationSelected: onDestinationSelected,
            trailing: Expanded(
              child: Column(
                mainAxisAlignment: MainAxisAlignment.end,
                children: [Text("v$version"), const SizedBox(height: 50)],
              ),
            ),
            leading: showAsDrawer
                ? Image.asset("assets/10101_flat_logo.png", width: 200, height: 50)
                : Image.asset("assets/10101_logo_icon.png", width: 50, height: 50),
            labelType: showAsDrawer ? NavigationRailLabelType.none : NavigationRailLabelType.all,
            destinations: destinations
                .map(
                  (navigation) => NavigationRailDestination(
                      label: Text(navigation.label),
                      icon: navigation.icon,
                      selectedIcon: navigation.selectedIcon),
                )
                .toList(),
          ),
          const VerticalDivider(thickness: 1, width: 1),
          // This is the main content.
          Expanded(
            child: Column(
              children: [
                Container(
                  decoration: const BoxDecoration(
                      color: Colors.white,
                      border: Border(bottom: BorderSide(width: 0.5, color: Colors.grey))),
                  padding: const EdgeInsets.all(25),
                  child: Row(
                    children: [
                      TopBarItem(label: 'Latest Bid: ', value: [
                        TextSpan(
                          text: bestQuote?.bid?.toString(),
                          style: const TextStyle(fontWeight: FontWeight.bold),
                        )
                      ]),
                      const SizedBox(width: 30),
                      TopBarItem(label: 'Latest Ask: ', value: [
                        TextSpan(
                          text: bestQuote?.ask?.toString(),
                          style: const TextStyle(fontWeight: FontWeight.bold),
                        )
                      ]),
                      const SizedBox(width: 30),
                      TopBarItem(label: 'Off-chain: ', value: [
                        TextSpan(
                            text: balance.offChain.formatted(),
                            style: const TextStyle(fontWeight: FontWeight.bold)),
                        const TextSpan(text: " sats"),
                      ]),
                      const SizedBox(width: 30),
                      TopBarItem(label: 'On-chain: ', value: [
                        TextSpan(
                            text: balance.onChain.formatted(),
                            style: const TextStyle(fontWeight: FontWeight.bold)),
                        const TextSpan(text: " sats"),
                      ]),
                    ],
                  ),
                ),
                Expanded(
                  child: body,
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

class TopBarItem extends StatelessWidget {
  final String label;
  final List<InlineSpan> value;

  const TopBarItem({super.key, required this.label, required this.value});

  @override
  Widget build(BuildContext context) {
    return value.isEmpty
        ? Row(
            mainAxisAlignment: MainAxisAlignment.center,
            children: <Widget>[
              Text(
                label,
                style: const TextStyle(color: Colors.black),
              ),
              const SizedBox(width: 10),
              const SizedBox(
                width: 20,
                height: 20,
                child: CircularProgressIndicator(),
              ),
            ],
          )
        : RichText(
            text: TextSpan(
              text: label,
              style: const TextStyle(fontSize: 16, color: Colors.black),
              children: value,
            ),
          );
  }
}
